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

#[derive(Debug, Default)]
pub struct RenameIndex {
    skill_renames: HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    subagent_renames: HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    renamed_agents: HashSet<(SourceName, ItemName)>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use crate::lock::ItemId;
    use crate::resolve::ResolvedGraph;
    use crate::sync::target::{CollisionRename, ExplicitRename, TargetItem, TargetState};
    use crate::types::SourceId;
    use indexmap::IndexMap;
    use mars_schema::item::ItemKind;
    use std::fs;
    use tempfile::TempDir;

    fn test_item(
        kind: ItemKind,
        name: &str,
        source_name: &str,
        source_path: std::path::PathBuf,
        dest_path: &str,
    ) -> TargetItem {
        let source_hash = if kind == ItemKind::Skill {
            hash::compute_hash(&source_path, kind).unwrap().into()
        } else {
            hash::hash_bytes(fs::read(&source_path).unwrap().as_slice()).into()
        };

        TargetItem {
            id: ItemId {
                kind,
                name: name.into(),
            },
            source_name: source_name.into(),
            source_path,
            dest_path: dest_path.into(),
            source_hash,
            is_flat_skill: false,
            rewritten_content: None,
        }
    }

    fn graph_with_deps(
        root: &std::path::Path,
        source_name: &str,
        deps: Vec<&str>,
    ) -> ResolvedGraph {
        let mut nodes = IndexMap::new();
        nodes.insert(
            SourceName::from(source_name),
            crate::resolve::ResolvedNode {
                fetch_url: None,
                source_name: source_name.into(),
                source_id: SourceId::Path {
                    canonical: root.to_path_buf(),
                    subpath: None,
                    plugin: None,
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: root.to_path_buf(),
                    authored_root: root.to_path_buf(),
                    package_root: root.to_path_buf(),
                    plugin: None,
                },
                prepared: crate::resolve::prepared_with_deps(&deps),
                resolved_ref: crate::source::ResolvedRef {
                    source_name: source_name.into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: root.to_path_buf(),
                },
                manifest: None,
            },
        );
        ResolvedGraph {
            nodes,
            order: vec![source_name.into()],
            selection: Default::default(),
            version_constraints: std::collections::HashMap::new(),
            unreadable_hook_surfaces: std::collections::BTreeMap::new(),
        }
    }

    fn apply_test_renames(
        target: &mut TargetState,
        explicit_skill_renames: &[ExplicitRename],
        collision_renames: &[CollisionRename],
        graph: &ResolvedGraph,
        dep_precedence: &[SourceName],
    ) {
        let index = RenameIndex::new(explicit_skill_renames, collision_renames, target);
        apply_renames(target, &index, graph, dep_precedence).unwrap();
    }

    #[test]
    fn collision_does_not_retarget_existing_same_source_ref_to_dep() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("source-a/agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills: [planning]\n---\n# Agent\n").unwrap();

        let source_a_skill_path = dir.path().join("source-a/skills/planning");
        let source_b_skill_path = dir.path().join("source-b/skills/planning");
        let source_c_skill_path = dir.path().join("source-c/skills/planning");
        fs::create_dir_all(&source_a_skill_path).unwrap();
        fs::create_dir_all(&source_b_skill_path).unwrap();
        fs::create_dir_all(&source_c_skill_path).unwrap();
        fs::write(source_a_skill_path.join("SKILL.md"), "# Planning A").unwrap();
        fs::write(source_b_skill_path.join("SKILL.md"), "# Planning B").unwrap();
        fs::write(source_c_skill_path.join("SKILL.md"), "# Planning C").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            test_item(
                ItemKind::Agent,
                "coder",
                "source-a",
                agent_path,
                "agents/coder.md",
            ),
        );
        items.insert(
            "skills/planning".into(),
            test_item(
                ItemKind::Skill,
                "planning",
                "source-a",
                source_a_skill_path,
                "skills/planning",
            ),
        );
        items.insert(
            "skills/planning__source-b".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-b",
                "source-b",
                source_b_skill_path,
                "skills/planning__source-b",
            ),
        );
        items.insert(
            "skills/planning__source-c".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-c",
                "source-c",
                source_c_skill_path,
                "skills/planning__source-c",
            ),
        );

        let mut target = TargetState::from_items(items);
        let renames = vec![
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-b".into(),
                source_name: "source-b".into(),
                kind: ItemKind::Skill,
            },
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-c".into(),
                source_name: "source-c".into(),
                kind: ItemKind::Skill,
            },
        ];
        let graph = graph_with_deps(dir.path(), "source-a", vec!["source-b"]);

        apply_test_renames(&mut target, &[], &renames, &graph, &[]);

        assert!(target.items["agents/coder.md"].rewritten_content.is_none());
    }
}
