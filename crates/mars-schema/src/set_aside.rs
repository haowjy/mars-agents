//! The class guard for translated definitions. After a foreign or pre-v1 definition is
//! translated, whatever the strict v1 reader would still reject is set aside, so a package its
//! consumer cannot edit never fails for it. Only an authored schema-v1 file is refused for its
//! fields.
//!
//! - A key Mars lowers itself (`LoweredField`, `skill::LOWERED_KEYS`) is never moved: native lowering would keep Mars's
//!   value and drop the override. A mapping or list loses only its rejected entries; a scalar, or
//!   a container the reader rejects as a whole, is dropped.
//! - A host set (`network-allow`, `network-deny`) loses only its rejected entries too, though
//!   Mars does not lower it: moving the whole list under `harness-overrides` would drop its valid
//!   hosts, and a lost deny host widens the agent.
//! - Any other rejected key is moved under `harness-overrides.<harness>` when a harness could
//!   read it, and dropped when none could.
//! - Losing a restriction (a `deny` or `ask` tool entry, `permission`, `sandbox`, `approval`,
//!   `network`, `network-deny`) is
//!   one of the changes like any other, with its own code ([`RESTRICTION_LOST`]) that names the
//!   agent. `mars migrate` lists it with the rest, and a sync prints it on every run
//!   (`Prepared::lost_restrictions`).

use std::cmp::Ordering;

use serde_yaml::Value;

use crate::HarnessId;
use crate::agent::LoweredField;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::Frontmatter;
use crate::harness_overrides::{Moved, move_keys_under};
use crate::item::DefinitionKind;
use crate::skill;

/// Keys whose errors cannot be cured by moving them: the file's identity, and the block moves go
/// to.
const KEPT_KEYS: &[&str] = &["name", "description", "harness-overrides"];

/// The code of the change that records a lost restriction.
pub(crate) const RESTRICTION_LOST: &str = "field.restriction-lost";

/// Keys that restrict what an agent may do; losing one widens the agent.
const RESTRICTION_KEYS: &[&str] = &[
    "permission",
    "sandbox",
    "approval",
    "network",
    "network-deny",
    "tools",
    "disallowed-tools",
];

/// Lists whose valid entries stay in place when one entry is rejected.
const HOST_SET_KEYS: &[&str] = &["network-allow", "network-deny"];

/// Where a rejected key goes.
#[derive(Clone, Copy)]
pub(crate) enum Destination<'a> {
    /// Under `harness-overrides.<name>`.
    Overrides(&'a str),
    /// Nowhere: the source has no harness whose keys these are.
    Nowhere,
}

impl Destination<'_> {
    /// The destination for a foreign dialect's keys: the harness that reads that dialect.
    pub(crate) fn for_dialect(dialect: crate::dialect::Dialect) -> Destination<'static> {
        use crate::dialect::Dialect;
        match dialect {
            Dialect::Claude => Destination::Overrides(HarnessId::Claude.as_str()),
            Dialect::Codex => Destination::Overrides(HarnessId::Codex.as_str()),
            Dialect::OpenCode => Destination::Overrides(HarnessId::OpenCode.as_str()),
            Dialect::Cursor => Destination::Overrides(HarnessId::Cursor.as_str()),
            Dialect::AgentSkills | Dialect::MarsNative => Destination::Nowhere,
        }
    }
}

/// What the strict reader refused: the path (its first segment is the key) and what it said.
struct Rejection {
    path: Vec<PathSegment>,
    why: String,
}

/// The keys of `fm` the strict reader rejects with an error, in file order, each with its
/// rejections.
fn rejected_keys(kind: DefinitionKind, fm: &Frontmatter) -> Vec<(String, Vec<Rejection>)> {
    let errors = crate::read::field_diagnostics(kind, fm)
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.is_error()
                && diagnostic.code != "field.required"
                && !diagnostic.code.starts_with("yaml.")
        })
        .filter(|diagnostic| matches!(diagnostic.path.first(), Some(PathSegment::Key(_))))
        .collect::<Vec<_>>();
    fm.keys()
        .into_iter()
        .filter(|key| !KEPT_KEYS.contains(&key.as_str()))
        .filter_map(|key| {
            let own = errors
                .iter()
                .filter(|diagnostic| matches!(diagnostic.path.first(), Some(PathSegment::Key(first)) if *first == key))
                .map(|diagnostic| Rejection {
                    path: diagnostic.path.clone(),
                    why: diagnostic.message.clone(),
                })
                .collect::<Vec<_>>();
            (!own.is_empty()).then_some((key, own))
        })
        .collect()
}

fn path_text(path: &[PathSegment]) -> String {
    path.iter()
        .map(|segment| match segment {
            PathSegment::Key(key) => key.clone(),
            PathSegment::Index(index) => index.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn compare_paths(a: &[PathSegment], b: &[PathSegment]) -> Ordering {
    for (left, right) in a.iter().zip(b) {
        let order = match (left, right) {
            (PathSegment::Key(left), PathSegment::Key(right)) => left.cmp(right),
            (PathSegment::Index(left), PathSegment::Index(right)) => left.cmp(right),
            (PathSegment::Key(_), PathSegment::Index(_)) => Ordering::Less,
            (PathSegment::Index(_), PathSegment::Key(_)) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// Remove the entry at `path` below `value`, and return it.
fn remove_at(value: &mut Value, path: &[PathSegment]) -> Option<Value> {
    match path {
        [] => None,
        [last] => match (value, last) {
            (Value::Mapping(mapping), PathSegment::Key(key)) => mapping.remove(key.as_str()),
            (Value::Sequence(sequence), PathSegment::Index(index)) if *index < sequence.len() => {
                Some(sequence.remove(*index))
            }
            _ => None,
        },
        [first, rest @ ..] => {
            let child = match (value, first) {
                (Value::Mapping(mapping), PathSegment::Key(key)) => mapping.get_mut(key.as_str()),
                (Value::Sequence(sequence), PathSegment::Index(index)) => sequence.get_mut(*index),
                _ => None,
            }?;
            remove_at(child, rest)
        }
    }
}

/// Drop the rejected entries of the mapping or list at `fm[key]` and keep the rest. Returns the
/// entries removed with their values, or `None` when `key` holds neither or an entry cannot be
/// found; `fm` is untouched then. A container left empty goes too.
fn drop_entries(
    fm: &mut Frontmatter,
    key: &str,
    rejections: &[Rejection],
) -> Option<Vec<(Vec<PathSegment>, Value)>> {
    let mut value = fm.get(key)?.clone();
    if !matches!(value, Value::Mapping(_) | Value::Sequence(_)) {
        return None;
    }
    let mut paths = rejections
        .iter()
        .map(|rejection| rejection.path.clone())
        .collect::<Vec<_>>();
    // Deepest and highest-index first, so a removal never moves an entry still to come.
    paths.sort_by(|a, b| compare_paths(b, a));
    paths.dedup();
    let mut removed = Vec::new();
    for path in paths {
        if path.len() < 2 {
            return None;
        }
        let entry = remove_at(&mut value, &path[1..])?;
        removed.push((path, entry));
    }
    let emptied = match &value {
        Value::Mapping(mapping) => mapping.is_empty(),
        Value::Sequence(sequence) => sequence.is_empty(),
        _ => false,
    };
    if emptied {
        fm.remove(key);
    } else {
        fm.insert(key, value);
    }
    Some(removed)
}

/// What was lost, when a restriction was: a `deny` or `ask` tool entry (or a tool-flag `false`),
/// an entry of `disallowed-tools`, a `network-deny` host, or a whole restriction key.
fn restriction_lost(path: &[PathSegment], removed: &Value) -> Option<String> {
    let Some(PathSegment::Key(key)) = path.first() else {
        return None;
    };
    if path.len() == 1 {
        return RESTRICTION_KEYS
            .contains(&key.as_str())
            .then(|| format!("`{key}`"));
    }
    let restricting = match key.as_str() {
        "tools" => {
            matches!(removed.as_str(), Some("deny" | "ask")) || *removed == Value::Bool(false)
        }
        "disallowed-tools" | "network-deny" => true,
        _ => false,
    };
    let value = match removed {
        Value::String(text) => text.clone(),
        other => crate::ordered::plain(other),
    };
    restricting.then(|| match key.as_str() {
        "tools" => format!("`{}: {value}`", path_text(path)),
        "network-deny" => format!("`network-deny: {value}`"),
        _ => format!("`{}`", path_text(path)),
    })
}

/// The reader's reason, except for a `permission` map: the reader's advice is about Flow's scalar
/// `permission`, and a map is OpenCode's format.
fn reason(key: &str, value: Option<&Value>, destination: Destination<'_>, read: &str) -> String {
    if key != "permission" || !matches!(value, Some(Value::Mapping(_))) {
        return read.to_owned();
    }
    match destination {
        Destination::Overrides(_) => "`permission` as a map is OpenCode's format; declare \
             `dialect = \"open-code\"` on this dependency so Mars reads it"
            .to_owned(),
        Destination::Nowhere => "`permission` as a map is OpenCode's format; put it under \
             `harness-overrides.opencode.permission`"
            .to_owned(),
    }
}

/// Reports what the guard did: a change per piece set aside, coded [`RESTRICTION_LOST`] when it
/// was a restriction.
struct Report<'a> {
    agent: String,
    changes: &'a mut Vec<SchemaDiagnostic>,
}

impl Report<'_> {
    /// `lost` is what a restriction-losing set-aside took away, if it did; `what` says what
    /// happened to the rejected piece, `why` is the reader's reason.
    fn note(&mut self, path: &[PathSegment], lost: Option<String>, what: &str, why: &str) {
        let shown = path_text(path);
        let (code, message) = match lost {
            Some(lost) => (
                RESTRICTION_LOST,
                format!(
                    "{} lost the restriction {lost}: {what}. Schema v1 does not accept it ({why})",
                    self.agent
                ),
            ),
            None => (
                "field.set-aside",
                format!("`{shown}` is not valid in schema v1 ({why}); {what}"),
            ),
        };
        self.changes.push(SchemaDiagnostic::new(
            code,
            Severity::Warning,
            path.to_vec(),
            message,
        ));
    }
}

/// Set aside every key of `fm` the strict reader rejects. Returns whether the frontmatter
/// changed. When `harness-overrides` or the destination entry is not a mapping the keys stay,
/// and the reader reports them.
pub(crate) fn set_aside_rejected_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    destination: Destination<'_>,
    changes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let rejected = rejected_keys(kind, fm);
    if rejected.is_empty() {
        return false;
    }
    let mut report = Report {
        agent: match fm.get("name").and_then(Value::as_str) {
            Some(name) => format!("agent `{name}`"),
            None => "this agent".to_owned(),
        },
        changes,
    };
    let mut changed = false;
    let mut movable = Vec::new();
    for (key, rejections) in rejected {
        let reserved = match kind {
            DefinitionKind::Agent => LoweredField::reserves(&key),
            DefinitionKind::Skill => skill::LOWERED_KEYS.contains(&key.as_str()),
        };
        let whole_key = rejections.iter().any(|rejection| rejection.path.len() == 1);
        if (reserved || HOST_SET_KEYS.contains(&key.as_str()))
            && !whole_key
            && let Some(removed) = drop_entries(fm, &key, &rejections)
        {
            changed = true;
            for (path, entry) in removed {
                let why = rejections
                    .iter()
                    .find(|rejection| rejection.path == path)
                    .map_or("", |rejection| rejection.why.as_str());
                let lost = restriction_lost(&path, &entry);
                let what = format!("that entry was dropped and the rest of `{key}` is kept");
                report.note(&path, lost, &what, why);
            }
            continue;
        }
        let why = reason(&key, fm.get(&key), destination, &rejections[0].why);
        if reserved || matches!(destination, Destination::Nowhere) {
            let removed = fm.remove(&key).unwrap_or(Value::Null);
            changed = true;
            let path = vec![PathSegment::Key(key.clone())];
            let lost = restriction_lost(&path, &removed);
            let what = if reserved {
                format!(
                    "it was dropped, because Mars lowers `{key}` itself and an override would not carry it"
                )
            } else {
                "it was dropped, because no harness reads it".to_owned()
            };
            report.note(&path, lost, &what, &why);
        } else {
            movable.push((key, why));
        }
    }
    if let Destination::Overrides(harness) = destination
        && !movable.is_empty()
    {
        let keys = movable
            .iter()
            .map(|(key, _)| (key.as_str(), key.as_str()))
            .collect::<Vec<_>>();
        if let Moved::Done { moved, displaced } = move_keys_under(fm, harness, &keys) {
            changed = true;
            for (key, why) in &movable {
                let what = if moved.contains(key) {
                    format!(
                        "it was moved under `harness-overrides.{harness}`, which Mars does not enforce"
                    )
                } else if displaced.contains(key) {
                    format!("it was dropped, `harness-overrides.{harness}.{key}` already sets it")
                } else {
                    continue;
                };
                let path = vec![PathSegment::Key(key.clone())];
                // A restriction moved under an override is no longer enforced by Mars.
                let lost = RESTRICTION_KEYS
                    .contains(&key.as_str())
                    .then(|| format!("`{key}`"));
                report.note(&path, lost, &what, why);
            }
        }
    }
    changed
}
