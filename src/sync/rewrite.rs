//! Unified frontmatter reference rewriting after renames.
//!
//! Explicit config renames and automatic collision renames are indexed together
//! before any agent content is rewritten. Each agent gets one rewrite pass that
//! resolves skill and subagent references against the post-prune target state,
//! so guards such as "same source still owns the unrenamed item" see only items
//! that will actually be installed.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::error::MarsError;
use crate::resolve::ResolvedGraph;
use crate::sync::target::{CollisionRename, ExplicitRename, TargetState};
use crate::types::{DestPath, SourceName};
use mars_schema::frontmatter;
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

type ContentRewriteFn =
    fn(&str, &IndexMap<String, String>) -> Result<Option<String>, frontmatter::FrontmatterError>;

/// All installed-name rewrites indexed by original item name.
#[derive(Debug, Default)]
pub struct RenameIndex {
    /// Skill original_name -> [(installed_name, source_name)].
    skill_renames: HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    /// Subagent original_name -> [(installed_name, source_name)].
    subagent_renames: HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    /// Installed agents whose authored `name:` must follow their destination.
    renamed_agents: HashSet<(SourceName, ItemName)>,
    /// Installed skills whose authored `name:` must follow their destination.
    renamed_skills: HashSet<(SourceName, ItemName)>,
}

impl RenameIndex {
    pub fn new(
        explicit_renames: &[ExplicitRename],
        collision_renames: &[CollisionRename],
        target: &TargetState,
    ) -> Self {
        let mut index = Self::default();
        let mut collision_final_names: HashMap<(SourceName, ItemKind, ItemName), ItemName> =
            HashMap::new();

        for rename in collision_renames {
            if !target_has_item(target, &rename.source_name, rename.kind, &rename.new_name) {
                continue;
            }

            let renames = match rename.kind {
                ItemKind::Skill => {
                    collision_final_names.insert(
                        (
                            rename.source_name.clone(),
                            rename.kind,
                            rename.original_name.clone(),
                        ),
                        rename.new_name.clone(),
                    );
                    index
                        .renamed_skills
                        .insert((rename.source_name.clone(), rename.new_name.clone()));
                    &mut index.skill_renames
                }
                ItemKind::Agent => {
                    collision_final_names.insert(
                        (
                            rename.source_name.clone(),
                            rename.kind,
                            rename.original_name.clone(),
                        ),
                        rename.new_name.clone(),
                    );
                    index
                        .renamed_agents
                        .insert((rename.source_name.clone(), rename.new_name.clone()));
                    &mut index.subagent_renames
                }
                _ => continue,
            };
            push_rename(
                renames,
                rename.original_name.clone(),
                rename.new_name.clone(),
                rename.source_name.clone(),
            );
        }

        for rename in explicit_renames {
            let installed_name = collision_final_names
                .get(&(
                    rename.source_name.clone(),
                    rename.kind,
                    rename.new_name.clone(),
                ))
                .unwrap_or(&rename.new_name);
            if !target_has_item(target, &rename.source_name, rename.kind, installed_name) {
                continue;
            }
            let renames = match rename.kind {
                ItemKind::Skill => {
                    index
                        .renamed_skills
                        .insert((rename.source_name.clone(), installed_name.clone()));
                    &mut index.skill_renames
                }
                ItemKind::Agent => {
                    index
                        .renamed_agents
                        .insert((rename.source_name.clone(), installed_name.clone()));
                    &mut index.subagent_renames
                }
                _ => continue,
            };
            push_rename(
                renames,
                rename.original_name.clone(),
                installed_name.clone(),
                rename.source_name.clone(),
            );
        }

        index
    }

    pub fn is_empty(&self) -> bool {
        self.skill_renames.is_empty()
            && self.subagent_renames.is_empty()
            && self.renamed_agents.is_empty()
            && self.renamed_skills.is_empty()
    }

    /// Installed-name rewrites visible from one source package.
    ///
    /// This is the same own-package-first, then dependency-order rule used by
    /// `apply_renames`. Plugin builds also need it for markdown body references,
    /// which are outside sync's canonical frontmatter rewrite.
    pub(crate) fn visible_renames(
        &self,
        target: &TargetState,
        graph: &ResolvedGraph,
        dep_precedence: &[SourceName],
        source_name: &SourceName,
        kind: ItemKind,
    ) -> IndexMap<String, String> {
        let source_deps = ordered_agent_deps(graph, source_name, dep_precedence);
        let renames = match kind {
            ItemKind::Skill => &self.skill_renames,
            ItemKind::Agent => &self.subagent_renames,
            _ => unreachable!("only agents and skills have rename references"),
        };
        renames_for_agent(target, source_name, &source_deps, kind, renames)
    }
}

/// Apply installed names to renamed skills, then rewrite agent references.
pub fn apply_renames(
    target: &mut TargetState,
    index: &RenameIndex,
    graph: &ResolvedGraph,
    dep_precedence: &[SourceName],
) -> Result<Vec<String>, MarsError> {
    let mut warnings = Vec::new();

    if index.is_empty() {
        return Ok(warnings);
    }

    for item in target.items.values_mut().filter(|item| {
        item.id.kind == ItemKind::Skill
            && index
                .renamed_skills
                .contains(&(item.source_name.clone(), item.id.name.clone()))
    }) {
        let skill_path = item.source_path.join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_path) {
            Ok(content) => content,
            Err(error) => {
                warnings.push(format!(
                    "warning: could not rewrite skill name in {}: {error}",
                    package_location(&item.source_name, &skill_path)
                ));
                continue;
            }
        };
        match frontmatter::parse(&content) {
            Ok(mut skill) => {
                skill.insert("name", serde_yaml::Value::String(item.id.name.to_string()));
                item.rewritten_content = Some(skill.render());
            }
            Err(error) => warnings.push(format!(
                "warning: could not rewrite skill name in {}: {error}",
                package_location(&item.source_name, &skill_path)
            )),
        }
    }

    // For each agent in target, check if it references any renamed items.
    let agent_keys: Vec<DestPath> = target
        .items
        .iter()
        .filter(|(_, item)| item.id.kind == ItemKind::Agent)
        .map(|(key, _)| key.clone())
        .collect();

    for key in agent_keys {
        let (source_path, source_name, installed_name, content) = {
            let item = &target.items[&key];
            let content = match &item.rewritten_content {
                Some(content) => content.clone(),
                None => match std::fs::read_to_string(&item.source_path) {
                    Ok(content) => content,
                    Err(_) => continue,
                },
            };
            (
                item.source_path.clone(),
                item.source_name.clone(),
                item.id.name.clone(),
                content,
            )
        };

        let skill_renames =
            index.visible_renames(target, graph, dep_precedence, &source_name, ItemKind::Skill);
        let subagent_renames =
            index.visible_renames(target, graph, dep_precedence, &source_name, ItemKind::Agent);
        let rewrite_name = index
            .renamed_agents
            .contains(&(source_name.clone(), installed_name.clone()));
        if skill_renames.is_empty() && subagent_renames.is_empty() && !rewrite_name {
            continue;
        }

        let mut rewritten_content = content;
        let mut changed = false;
        if rewrite_name {
            match frontmatter::parse(&rewritten_content) {
                Ok(mut frontmatter) => {
                    frontmatter.insert(
                        "name",
                        serde_yaml::Value::String(installed_name.to_string()),
                    );
                    rewritten_content = frontmatter.render();
                    changed = true;
                }
                Err(error) => warnings.push(format!(
                    "warning: could not rewrite agent name in {}: {error}",
                    package_location(&source_name, &source_path)
                )),
            }
        }
        rewrite_content_for_agent(
            &mut rewritten_content,
            &skill_renames,
            "skill",
            &source_path,
            frontmatter::rewrite_content_skills,
            &mut warnings,
            &mut changed,
        );
        rewrite_content_for_agent(
            &mut rewritten_content,
            &subagent_renames,
            "subagent",
            &source_path,
            frontmatter::rewrite_content_subagents,
            &mut warnings,
            &mut changed,
        );
        if changed && let Some(target_item) = target.items.get_mut(&key) {
            target_item.rewritten_content = Some(rewritten_content);
        }
    }

    Ok(warnings)
}

fn push_rename(
    renames: &mut HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    original_name: ItemName,
    new_name: ItemName,
    source_name: SourceName,
) {
    let entries = renames.entry(original_name).or_default();
    if entries.iter().any(|(existing_name, existing_source)| {
        existing_name == &new_name && existing_source == &source_name
    }) {
        return;
    }
    entries.push((new_name, source_name));
}

fn target_has_item(
    target: &TargetState,
    source_name: &SourceName,
    kind: ItemKind,
    name: &ItemName,
) -> bool {
    target.items.values().any(|item| {
        item.source_name == *source_name && item.id.kind == kind && item.id.name == *name
    })
}

fn ordered_agent_deps(
    graph: &ResolvedGraph,
    source_name: &SourceName,
    dep_precedence: &[SourceName],
) -> Vec<SourceName> {
    // The consumer's own package has no resolved node: its items may reference
    // every declared dependency, in declaration order.
    let Some(node) = graph.nodes.get(source_name) else {
        return dep_precedence.to_vec();
    };

    let mut ordered = Vec::new();
    for dep in dep_precedence {
        if node.deps().any(|candidate| candidate == *dep) {
            ordered.push(dep.clone());
        }
    }
    for dep in node.deps() {
        if !ordered.contains(&dep) {
            ordered.push(dep);
        }
    }
    ordered
}

fn renames_for_agent(
    target: &TargetState,
    source_name: &SourceName,
    agent_deps: &[SourceName],
    referenced_kind: ItemKind,
    renames: &HashMap<ItemName, Vec<(ItemName, SourceName)>>,
) -> IndexMap<String, String> {
    let mut renames_for_agent = IndexMap::new();
    for (original_name, entries) in renames {
        let selected = entries.iter().find(|(_, source)| source == source_name);
        let selected = if selected.is_none()
            && source_has_unrenamed_item(target, source_name, referenced_kind, original_name)
        {
            None
        } else {
            selected.or_else(|| {
                agent_deps
                    .iter()
                    .find_map(|dep| entries.iter().find(|(_, source)| source == dep))
            })
        };
        if let Some((new_name, _)) = selected {
            renames_for_agent.insert(original_name.to_string(), new_name.to_string());
        }
    }
    renames_for_agent
}

/// `agents/<file>` or `skills/<name>/SKILL.md` within its package: a path inside the cache or
/// an archive is no use to the person reading the warning.
fn package_relative(path: &std::path::Path) -> String {
    let parts: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts
        .iter()
        .rposition(|part| part == "agents" || part == "skills")
    {
        Some(at) => parts[at..].join("/"),
        None => parts.last().cloned().unwrap_or_default(),
    }
}

/// `<package>: agents/<file>`, naming the dependency instead of its cache location.
fn package_location(source_name: &SourceName, path: &std::path::Path) -> String {
    format!("{source_name}: {}", package_relative(path))
}

fn rewrite_content_for_agent(
    content: &mut String,
    renames: &IndexMap<String, String>,
    label: &str,
    source_path: &std::path::Path,
    rewrite_content: ContentRewriteFn,
    warnings: &mut Vec<String>,
    changed: &mut bool,
) {
    if renames.is_empty() {
        return;
    }

    match rewrite_content(content, renames) {
        Ok(Some(new_content)) => {
            *content = new_content;
            *changed = true;
        }
        Ok(None) => {}
        Err(e) => {
            warnings.push(format!(
                "warning: could not rewrite {label} refs in {}: {e}",
                package_relative(source_path)
            ));
        }
    }
}

fn source_has_unrenamed_item(
    target: &TargetState,
    source_name: &SourceName,
    kind: ItemKind,
    name: &ItemName,
) -> bool {
    target.items.values().any(|item| {
        item.source_name == *source_name && item.id.kind == kind && item.id.name == *name
    })
}
