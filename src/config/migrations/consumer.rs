//! `mars migrate` for the consumer config files: apply the key table's rewrites to
//! `mars.toml`, `mars.local.toml` and the user `config.toml`, and turn the retired
//! `allowed` / `disallowed` tool lists into rule entries.
//!
//! A rewrite never widens what an agent may do. Where it cannot be exact, the table is left as
//! it is, and the report says why and what the nearest rewrite would change.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Key, TableLike, value};

use crate::config::read::{Segment, surface, table_name};
use crate::error::{ConfigError, MarsError};
use mars_schema::keys::{self, Rewrite};
use mars_schema::tool_names::{Capability, OverlayToolName, ParsedToolName};
use mars_schema::tool_policy::{EffectiveToolPolicy, ToolAction};

/// One thing the migration did, or left for the author.
pub(crate) struct Notice {
    pub(crate) file: String,
    pub(crate) message: String,
}

pub(crate) struct Rewritten {
    pub(crate) path: PathBuf,
    pub(crate) file: String,
    pub(crate) text: String,
}

#[derive(Default)]
pub(crate) struct Outcome {
    pub(crate) rewritten: Vec<Rewritten>,
    /// What was rewritten or dropped.
    pub(crate) changes: Vec<Notice>,
    /// What stays in the file and fails the next load until the author edits it.
    pub(crate) left: Vec<Notice>,
}

/// Each installed agent's own tool rules, by name: what a rule entry replaces.
pub(crate) type Profiles = BTreeMap<String, EffectiveToolPolicy>;

struct Layer {
    file: String,
    path: PathBuf,
    text: String,
    doc: DocumentMut,
}

/// Migrate the project's `mars.toml` and `mars.local.toml` and the user's `config.toml`, from
/// lowest layer to highest. Nothing is written here.
pub(crate) fn migrate(
    project_root: &Path,
    user_config: Option<&Path>,
    profiles: &Profiles,
) -> Result<Outcome, MarsError> {
    let mut layers = Vec::new();
    let candidates = user_config
        .map(|path| (path.to_path_buf(), path.display().to_string()))
        .into_iter()
        .chain(["mars.toml", "mars.local.toml"].map(|name| (project_root.join(name), name.into())));
    for (path, file) in candidates {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(crate::config::read_error(&path, source)),
        };
        let doc = text
            .parse()
            .map_err(|error: toml_edit::TomlError| ConfigError::Invalid {
                message: format!("{file}: {error}"),
            })?;
        layers.push(Layer {
            file,
            path,
            text,
            doc,
        });
    }

    let mut outcome = Outcome::default();
    let mut stopped = HashSet::new();
    // Decisions read the files as the author wrote them, so one layer's rewrite never changes
    // what another is judged against.
    let originals: Vec<DocumentMut> = layers.iter().map(|layer| layer.doc.clone()).collect();
    for (index, layer) in layers.iter_mut().enumerate() {
        tool_lists(
            index,
            layer,
            &originals,
            profiles,
            &mut outcome,
            &mut stopped,
        );
        let mut at = Vec::new();
        rewrite_table(
            layer.doc.as_table_mut(),
            &mut at,
            &layer.file,
            &stopped,
            &mut outcome,
        );
    }
    for layer in layers {
        let text = layer.doc.to_string();
        if text != layer.text {
            outcome.rewritten.push(Rewritten {
                path: layer.path,
                file: layer.file,
                text,
            });
        }
    }
    Ok(outcome)
}

/// Apply every removal the table can rewrite on its own (drop, rename), and list the ones it
/// cannot.
fn rewrite_table(
    table: &mut dyn TableLike,
    at: &mut Vec<Segment>,
    file: &str,
    stopped: &HashSet<String>,
    outcome: &mut Outcome,
) {
    let names: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
    for name in names {
        let removal = surface(at).and_then(|surface| keys::removed_key(surface, &name));
        if let Some((_, removal)) = removal {
            let place = format!("[{}] `{name}`", table_name(at));
            match removal.rewrite {
                Rewrite::Drop => {
                    table.remove(&name);
                    outcome.changes.push(Notice {
                        file: file.into(),
                        message: format!("{place} removed; {}", removal.advice),
                    });
                }
                Rewrite::Rename(new) if !table.contains_key(new) => {
                    let leaf = table.key(&name).map(|key| key.leaf_decor().clone());
                    if let Some(item) = table.remove(&name) {
                        let mut key = Key::new(new);
                        if let Some(leaf) = leaf {
                            *key.leaf_decor_mut() = leaf;
                        }
                        table.entry_format(&key).or_insert(item);
                    }
                    outcome.changes.push(Notice {
                        file: file.into(),
                        message: format!("{place} renamed to `{new}`"),
                    });
                }
                Rewrite::Rename(new) => outcome.left.push(Notice {
                    file: file.into(),
                    message: format!("{place} sits beside `{new}`; remove one of them"),
                }),
                Rewrite::None | Rewrite::Value(_) => {
                    if !stopped.contains(&stop_key(file, at, &name)) {
                        outcome.left.push(Notice {
                            file: file.into(),
                            message: format!("{place} was removed; {}", removal.advice),
                        });
                    }
                }
            }
            continue;
        }
        if !at.is_empty() || matches!(name.as_str(), "settings" | "agents" | "skills") {
            let Some(item) = table.get_mut(&name) else {
                continue;
            };
            at.push(Segment::Key(name.clone()));
            match item {
                Item::ArrayOfTables(tables) => {
                    for (index, inner) in tables.iter_mut().enumerate() {
                        at.push(Segment::Index(index));
                        rewrite_table(inner, at, file, stopped, outcome);
                        at.pop();
                    }
                }
                other => {
                    if let Some(inner) = other.as_table_like_mut() {
                        rewrite_table(inner, at, file, stopped, outcome);
                    }
                }
            }
            at.pop();
        }
    }
}

fn stop_key(file: &str, at: &[Segment], key: &str) -> String {
    format!("{file}\0{}\0{key}", table_name(at))
}

#[derive(Clone)]
enum List {
    Add(Vec<String>),
    Replace(Vec<String>),
}

impl List {
    fn read(item: &Item) -> Option<Self> {
        let strings = |item: &Item| -> Option<Vec<String>> {
            item.as_array()?
                .iter()
                .map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        };
        match item.as_array() {
            Some(_) => strings(item).map(Self::Add),
            None => strings(item.as_table_like()?.get("replace")?).map(Self::Replace),
        }
    }

    fn entries(&self) -> &[String] {
        match self {
            Self::Add(entries) | Self::Replace(entries) => entries,
        }
    }
}

fn key_of(name: &str) -> Option<ParsedToolName> {
    OverlayToolName::try_from(name.to_owned())
        .ok()
        .map(|name| name.parsed().clone())
}

/// The strictest action any layer, the agent's profile or `[settings.tools]` gives `key` for
/// this agent, as it stands before the migration.
fn strictest(
    key: &ParsedToolName,
    agent: &str,
    originals: &[DocumentMut],
    profile: &EffectiveToolPolicy,
) -> Option<ToolAction> {
    let from_profile = profile
        .rules
        .iter()
        .filter(|rule| rule.capability == key.capability && rule.pattern == key.pattern)
        .map(|rule| rule.action);
    let mut from_files = Vec::new();
    for doc in originals {
        let tables = [
            doc.get("settings")
                .and_then(|settings| settings.get("tools"))
                .and_then(Item::as_table_like),
            agent_tools(doc, agent),
        ];
        for (index, table) in tables.into_iter().enumerate() {
            for (name, item) in table.into_iter().flat_map(|table| table.iter()) {
                let action = item.as_str().and_then(parse_action);
                if let (Some(action), Some(named)) = (action, key_of(name))
                    && named == *key
                {
                    from_files.push(action);
                }
                // The agent's `disallowed` list is a deny of each name in it.
                if index == 1
                    && name == "disallowed"
                    && let Some(list) = List::read(item)
                    && list
                        .entries()
                        .iter()
                        .any(|entry| key_of(entry).as_ref() == Some(key))
                {
                    from_files.push(ToolAction::Deny);
                }
            }
        }
    }
    from_profile
        .chain(from_files)
        .max_by_key(|action| action.strictness())
}

fn parse_action(text: &str) -> Option<ToolAction> {
    [ToolAction::Allow, ToolAction::Ask, ToolAction::Deny]
        .into_iter()
        .find(|action| action.as_str() == text)
}

fn agent_tools<'a>(doc: &'a DocumentMut, agent: &str) -> Option<&'a dyn TableLike> {
    doc.get("agents")?.get(agent)?.get("tools")?.as_table_like()
}

/// Rewrite the `allowed` / `disallowed` lists of one layer into rule entries.
fn tool_lists(
    index: usize,
    layer: &mut Layer,
    originals: &[DocumentMut],
    profiles: &Profiles,
    outcome: &mut Outcome,
    stopped: &mut HashSet<String>,
) {
    let agents: Vec<String> = originals[index]
        .get("agents")
        .and_then(Item::as_table_like)
        .map(|agents| agents.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default();
    for agent in agents {
        for list_name in ["disallowed", "allowed"] {
            let Some(list) = agent_tools(&originals[index], &agent)
                .and_then(|tools| tools.get(list_name))
                .map(List::read)
            else {
                continue;
            };
            let table = format!("agents.{agent}.tools");
            let place = format!("[{table}] `{list_name}`");
            let result = match list {
                Some(list) => {
                    let lower = &originals[..index];
                    convert(&agent, list_name, &list, lower, originals, profiles)
                }
                None => Err("the list is not an array of strings".to_owned()),
            };
            match result {
                Ok(done) => {
                    apply(&mut layer.doc, &agent, list_name, &done.rules);
                    outcome.changes.push(Notice {
                        file: layer.file.clone(),
                        message: format!(
                            "{place} rewritten as {}",
                            if done.rules.is_empty() {
                                "no rules".to_owned()
                            } else {
                                done.rules
                                    .iter()
                                    .map(|(name, action)| format!("`{name}` = \"{action}\""))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        ),
                    });
                    for dropped in done.dropped {
                        outcome.changes.push(Notice {
                            file: layer.file.clone(),
                            message: format!("{place} entry dropped: {dropped}"),
                        });
                    }
                }
                Err(reason) => {
                    stopped.insert(format!("{}\0{table}\0{list_name}", layer.file));
                    outcome.left.push(Notice {
                        file: layer.file.clone(),
                        message: format!("{place} left as it is: {reason}"),
                    });
                }
            }
        }
    }
}

struct Converted {
    rules: Vec<(String, &'static str)>,
    /// Entries a stricter rule already beats, with that rule.
    dropped: Vec<String>,
}

/// The rule entries that give an agent the same access as one list of its overlay, or why there
/// are none.
fn convert(
    agent: &str,
    list_name: &str,
    list: &List,
    lower_layers: &[DocumentMut],
    originals: &[DocumentMut],
    profiles: &Profiles,
) -> Result<Converted, String> {
    let profile = profiles.get(agent);
    let mut converted = Converted {
        rules: Vec::new(),
        dropped: Vec::new(),
    };
    for entry in list.entries() {
        if key_of(entry).is_none() {
            return Err(format!("`{entry}` is not a tool name"));
        }
    }
    if list_name == "disallowed" {
        // `K = "deny"` is the strictest action: it replaces whatever the profile and
        // `[settings.tools]` say about K, and nothing beats it.
        if let List::Replace(_) = list
            && let Some(cleared) = profile.map(|profile| profile_keys(profile, ToolAction::Deny))
            && !cleared.is_empty()
        {
            return Err(format!(
                "`replace` also removes the profile's deny rules ({}), and a rule entry cannot remove a rule; write the denies as `\"<tool>\" = \"deny\"` and decide about those by hand",
                cleared.join(", ")
            ));
        }
        converted.rules = list
            .entries()
            .iter()
            .map(|entry| (entry.clone(), "deny"))
            .collect();
        return Ok(converted);
    }

    let Some(profile) = profile else {
        return Err(format!(
            "no installed profile for agent `{agent}` (run `mars sync` first), so whether a stricter rule beats an entry cannot be told"
        ));
    };
    let denies_all = profile
        .rules
        .iter()
        .any(|rule| rule.capability == Capability::Any && rule.action == ToolAction::Deny);
    if let List::Replace(_) = list {
        if !denies_all {
            return Err(format!(
                "`replace` removes the profile's allows ({}) and its `'*'` rule, and agent `{agent}` has no `'*': deny` for the removal to fall back on. Nearest rewrite: `\"<tool>\" = \"allow\"` for each listed tool, which keeps the profile's other allows",
                profile_keys(profile, ToolAction::Allow).join(", ")
            ));
        }
        // On a `'*': deny` agent a dropped allow is a deny: the profile's allows and the allows
        // that lower layers' lists add, except those this list names again.
        let named: Vec<ParsedToolName> = list
            .entries()
            .iter()
            .filter_map(|entry| key_of(entry))
            .collect();
        let mut cleared = profile_keys(profile, ToolAction::Allow);
        for doc in lower_layers {
            if let Some(lower) = agent_tools(doc, agent)
                .and_then(|tools| tools.get("allowed"))
                .and_then(List::read)
            {
                cleared.extend(lower.entries().iter().cloned());
            }
        }
        for name in cleared {
            // A rule entry applies after the lists, so one that names the tool survives `replace`.
            if key_of(&name)
                .is_some_and(|key| !named.contains(&key) && !has_rule_entry(originals, agent, &key))
                && !converted.rules.iter().any(|(held, _)| *held == name)
            {
                converted.rules.push((name, "deny"));
            }
        }
    }
    for entry in list.entries() {
        let key = key_of(entry).expect("checked above");
        match strictest(&key, agent, originals, profile) {
            Some(action) if action.strictness() > ToolAction::Allow.strictness() => {
                converted.dropped.push(format!(
                    "`{entry}` is already `{}` for this agent, and the stricter rule decides; as a rule entry `allow` would replace it",
                    action.as_str()
                ));
            }
            _ => converted.rules.push((entry.clone(), "allow")),
        }
    }
    Ok(converted)
}

fn has_rule_entry(originals: &[DocumentMut], agent: &str, key: &ParsedToolName) -> bool {
    originals.iter().any(|doc| {
        agent_tools(doc, agent).is_some_and(|tools| {
            tools.iter().any(|(name, item)| {
                item.as_str().and_then(parse_action).is_some() && key_of(name).as_ref() == Some(key)
            })
        })
    })
}

fn profile_keys(profile: &EffectiveToolPolicy, action: ToolAction) -> Vec<String> {
    profile
        .rules
        .iter()
        .filter(|rule| rule.action == action && rule.capability != Capability::Any)
        .map(|rule| rule.render())
        .collect()
}

/// Remove `list_name` from the agent's tools table and add the rule entries, keeping a key that
/// is already there when it is the stricter.
fn apply(doc: &mut DocumentMut, agent: &str, list_name: &str, rules: &[(String, &'static str)]) {
    let Some(tools) = doc
        .get_mut("agents")
        .and_then(|agents| agents.get_mut(agent))
        .and_then(|agent| agent.get_mut("tools"))
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    tools.remove(list_name);
    for (name, action) in rules {
        let key = key_of(name);
        let existing = tools
            .iter()
            .find(|(held, _)| key_of(held) == key)
            .map(|(held, item)| (held.to_owned(), item.as_str().and_then(parse_action)));
        match existing {
            Some((_, Some(held)))
                if held.strictness()
                    >= parse_action(action).expect("a rule action").strictness() => {}
            Some((held, _)) => {
                tools.insert(&held, value(*action));
            }
            None => {
                tools.insert(name, value(*action));
            }
        }
    }
}
