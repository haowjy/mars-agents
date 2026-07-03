//! Frontmatter reference rewriting after renames.
//!
//! When explicit config or automatic collision handling renames an item,
//! affected agents may still reference the original name in frontmatter. This
//! module rewrites those scoped references to point at the installed name.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::error::MarsError;
use crate::frontmatter;
use crate::lock::ItemKind;
use crate::resolve::ResolvedGraph;
use crate::sync::target::{CollisionRename, ExplicitSkillRename, TargetState};
use crate::types::{DestPath, ItemName, SourceName};

type ContentRewriteFn =
    fn(&str, &IndexMap<String, String>) -> Result<Option<String>, frontmatter::FrontmatterError>;

/// Rewrite frontmatter skill references for renamed transitive deps.
///
/// When a collision forces a rename AND affected agents have frontmatter
/// `skills:` references to the renamed skill, mars rewrites those references
/// to point at the correct renamed version.
pub fn rewrite_skill_refs(
    target: &mut TargetState,
    renames: &[ExplicitSkillRename],
    graph: &ResolvedGraph,
) -> Result<Vec<String>, MarsError> {
    if renames.is_empty() {
        return Ok(Vec::new());
    }

    // ExplicitSkillRename is only recorded for skills:
    // original skill name -> [(renamed skill name, source name)].
    let mut skill_renames: HashMap<ItemName, Vec<(ItemName, SourceName)>> = HashMap::new();
    for ra in renames {
        skill_renames
            .entry(ra.original_name.clone())
            .or_default()
            .push((ra.new_name.clone(), ra.source_name.clone()));
    }

    rewrite_agent_frontmatter_refs(
        target,
        &skill_renames,
        graph,
        ItemKind::Skill,
        "skill",
        frontmatter::rewrite_content_skills,
    )
}

/// Rewrite frontmatter references after automatic collision renames.
pub fn rewrite_collision_refs(
    target: &mut TargetState,
    renames: &[CollisionRename],
    graph: &ResolvedGraph,
) -> Result<Vec<String>, MarsError> {
    let mut warnings = Vec::new();

    if renames.is_empty() {
        return Ok(warnings);
    }

    let skill_renames = grouped_collision_renames(renames, ItemKind::Skill);
    warnings.extend(rewrite_agent_frontmatter_refs(
        target,
        &skill_renames,
        graph,
        ItemKind::Skill,
        "skill",
        frontmatter::rewrite_content_skills,
    )?);

    let agent_renames = grouped_collision_renames(renames, ItemKind::Agent);
    warnings.extend(rewrite_agent_frontmatter_refs(
        target,
        &agent_renames,
        graph,
        ItemKind::Agent,
        "subagent",
        frontmatter::rewrite_content_subagents,
    )?);

    Ok(warnings)
}

fn grouped_collision_renames(
    renames: &[CollisionRename],
    kind: ItemKind,
) -> HashMap<ItemName, Vec<(ItemName, SourceName)>> {
    let mut grouped = HashMap::new();
    for rename in renames.iter().filter(|rename| rename.kind == kind) {
        grouped
            .entry(rename.original_name.clone())
            .or_insert_with(Vec::new)
            .push((rename.new_name.clone(), rename.source_name.clone()));
    }
    grouped
}

fn rewrite_agent_frontmatter_refs(
    target: &mut TargetState,
    renames: &HashMap<ItemName, Vec<(ItemName, SourceName)>>,
    graph: &ResolvedGraph,
    referenced_kind: ItemKind,
    label: &str,
    rewrite_content: ContentRewriteFn,
) -> Result<Vec<String>, MarsError> {
    let mut warnings = Vec::new();

    if renames.is_empty() {
        return Ok(warnings);
    }

    // For each agent in target, check if it references any renamed items.
    let agent_keys: Vec<DestPath> = target
        .items
        .iter()
        .filter(|(_, item)| item.id.kind == ItemKind::Agent)
        .map(|(key, _)| key.clone())
        .collect();

    for key in agent_keys {
        let (source_path, source_name, content) = {
            let item = &target.items[&key];
            let content = match &item.rewritten_content {
                Some(content) => content.clone(),
                None => match std::fs::read_to_string(&item.source_path) {
                    Ok(content) => content,
                    Err(_) => continue,
                },
            };
            (item.source_path.clone(), item.source_name.clone(), content)
        };

        let mut renames_for_agent: IndexMap<String, String> = IndexMap::new();
        let agent_deps: &[SourceName] = if let Some(node) = graph.nodes.get(&source_name) {
            node.deps.as_slice()
        } else if source_name.as_str() == "_self" {
            graph.order.as_slice()
        } else {
            &[]
        };

        for (original_name, entries) in renames {
            let selected = entries.iter().find(|(_, source)| source == &source_name);
            let selected = if selected.is_none()
                && source_has_unrenamed_item(target, &source_name, referenced_kind, original_name)
            {
                None
            } else {
                selected.or_else(|| {
                    entries
                        .iter()
                        .find(|(_, source)| agent_deps.contains(source))
                })
            };
            if let Some((new_name, _)) = selected {
                renames_for_agent.insert(original_name.to_string(), new_name.to_string());
            }
        }
        if renames_for_agent.is_empty() {
            continue;
        }

        match rewrite_content(&content, &renames_for_agent) {
            Ok(Some(new_content)) => {
                if let Some(target_item) = target.items.get_mut(&key) {
                    target_item.rewritten_content = Some(new_content);
                }
            }
            Ok(None) => {}
            Err(e) => {
                warnings.push(format!(
                    "warning: could not rewrite {label} refs in {}: {e}",
                    source_path.display()
                ));
            }
        }
    }

    Ok(warnings)
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
    use crate::lock::{ItemId, ItemKind};
    use crate::resolve::ResolvedGraph;
    use crate::sync::target::{CollisionRename, ExplicitSkillRename, TargetItem, TargetState};
    use crate::types::SourceId;
    use indexmap::IndexMap;
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
            origin: crate::types::SourceOrigin::Dependency(source_name.into()),
            source_id: SourceId::Path {
                canonical: source_path.clone(),
                subpath: None,
            },
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
                source_name: source_name.into(),
                source_id: SourceId::Path {
                    canonical: root.to_path_buf(),
                    subpath: None,
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: root.to_path_buf(),
                    package_root: root.to_path_buf(),
                },
                resolved_ref: crate::source::ResolvedRef {
                    source_name: source_name.into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: root.to_path_buf(),
                },
                latest_version: None,
                manifest: None,
                deps: deps.into_iter().map(SourceName::from).collect(),
            },
        );
        ResolvedGraph {
            nodes,
            order: vec![source_name.into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn rewrite_skill_refs_uses_exact_skill_matches() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(
            &agent_path,
            "---\nskills:\n- plan\n- planner\n---\n# Agent\n",
        )
        .unwrap();

        let skill_path = dir.path().join("skills/plan__org_base");
        fs::create_dir_all(&skill_path).unwrap();
        fs::write(skill_path.join("SKILL.md"), "# Planning").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                source_name: "source-a".into(),
                origin: crate::types::SourceOrigin::Dependency("source-a".into()),
                source_id: SourceId::Path {
                    canonical: agent_path.clone(),
                    subpath: None,
                },
                source_path: agent_path.clone(),
                dest_path: "agents/coder.md".into(),
                source_hash: hash::hash_bytes(fs::read(&agent_path).unwrap().as_slice()).into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );
        items.insert(
            "skills/plan__org_base".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Skill,
                    name: "plan__org_base".into(),
                },
                source_name: "source-a".into(),
                origin: crate::types::SourceOrigin::Dependency("source-a".into()),
                source_id: SourceId::Path {
                    canonical: skill_path.clone(),
                    subpath: None,
                },
                source_path: skill_path.clone(),
                dest_path: "skills/plan__org_base".into(),
                source_hash: hash::compute_hash(&skill_path, ItemKind::Skill)
                    .unwrap()
                    .into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );

        let mut target = TargetState { items };
        let renames = vec![ExplicitSkillRename {
            original_name: "plan".into(),
            new_name: "plan__org_base".into(),
            source_name: "source-a".into(),
        }];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec![],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_skill_refs(&mut target, &renames, &graph).unwrap();

        let rewritten = target.items["agents/coder.md"]
            .rewritten_content
            .as_ref()
            .unwrap();
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        assert_eq!(fm.skills(), vec!["plan__org_base", "planner"]);
    }

    #[test]
    fn rewrite_skill_refs_leaves_non_matching_agents_unchanged() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills: [review]\n---\n# Agent\n").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                source_name: "source-a".into(),
                origin: crate::types::SourceOrigin::Dependency("source-a".into()),
                source_id: SourceId::Path {
                    canonical: agent_path.clone(),
                    subpath: None,
                },
                source_path: agent_path.clone(),
                dest_path: "agents/coder.md".into(),
                source_hash: hash::hash_bytes(fs::read(&agent_path).unwrap().as_slice()).into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );

        let mut target = TargetState { items };
        let renames = vec![ExplicitSkillRename {
            original_name: "plan".into(),
            new_name: "plan__org_base".into(),
            source_name: "source-a".into(),
        }];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec![],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_skill_refs(&mut target, &renames, &graph).unwrap();
        assert!(target.items["agents/coder.md"].rewritten_content.is_none());
    }

    #[test]
    fn rewrite_skill_refs_cross_package_uses_dep_graph() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills:\n- planning\n---\n# Agent\n").unwrap();

        let skill_b_path = dir.path().join("skills/planning__org_b");
        fs::create_dir_all(&skill_b_path).unwrap();
        fs::write(skill_b_path.join("SKILL.md"), "# Planning from B").unwrap();

        let skill_c_path = dir.path().join("skills/planning__org_c");
        fs::create_dir_all(&skill_c_path).unwrap();
        fs::write(skill_c_path.join("SKILL.md"), "# Planning from C").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                source_name: "source-a".into(),
                origin: crate::types::SourceOrigin::Dependency("source-a".into()),
                source_id: SourceId::Path {
                    canonical: agent_path.clone(),
                    subpath: None,
                },
                source_path: agent_path.clone(),
                dest_path: "agents/coder.md".into(),
                source_hash: hash::hash_bytes(fs::read(&agent_path).unwrap().as_slice()).into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );
        items.insert(
            "skills/planning__org_b".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Skill,
                    name: "planning__org_b".into(),
                },
                source_name: "source-b".into(),
                origin: crate::types::SourceOrigin::Dependency("source-b".into()),
                source_id: SourceId::Path {
                    canonical: skill_b_path.clone(),
                    subpath: None,
                },
                source_path: skill_b_path.clone(),
                dest_path: "skills/planning__org_b".into(),
                source_hash: hash::compute_hash(&skill_b_path, ItemKind::Skill)
                    .unwrap()
                    .into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );
        items.insert(
            "skills/planning__org_c".into(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Skill,
                    name: "planning__org_c".into(),
                },
                source_name: "source-c".into(),
                origin: crate::types::SourceOrigin::Dependency("source-c".into()),
                source_id: SourceId::Path {
                    canonical: skill_c_path.clone(),
                    subpath: None,
                },
                source_path: skill_c_path.clone(),
                dest_path: "skills/planning__org_c".into(),
                source_hash: hash::compute_hash(&skill_c_path, ItemKind::Skill)
                    .unwrap()
                    .into(),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );

        let mut target = TargetState { items };
        let renames = vec![
            ExplicitSkillRename {
                original_name: "planning".into(),
                new_name: "planning__org_b".into(),
                source_name: "source-b".into(),
            },
            ExplicitSkillRename {
                original_name: "planning".into(),
                new_name: "planning__org_c".into(),
                source_name: "source-c".into(),
            },
        ];

        let mut nodes = IndexMap::new();
        nodes.insert(
            SourceName::from("source-a"),
            crate::resolve::ResolvedNode {
                source_name: "source-a".into(),
                source_id: SourceId::Path {
                    canonical: dir.path().to_path_buf(),
                    subpath: None,
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: dir.path().to_path_buf(),
                    package_root: dir.path().to_path_buf(),
                },
                resolved_ref: crate::source::ResolvedRef {
                    source_name: "source-a".into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: dir.path().to_path_buf(),
                },
                latest_version: None,
                manifest: None,
                deps: vec!["source-b".into()],
            },
        );
        let graph = ResolvedGraph {
            nodes,
            order: vec!["source-a".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_skill_refs(&mut target, &renames, &graph).unwrap();

        let rewritten = target.items["agents/coder.md"]
            .rewritten_content
            .as_ref()
            .expect("agent should have been rewritten");
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        assert_eq!(fm.skills(), vec!["planning__org_b"]);
    }

    #[test]
    fn collision_rewrites_subagent_refs() {
        let dir = TempDir::new().unwrap();
        let source_a_agents = dir.path().join("source-a/agents");
        let source_b_agents = dir.path().join("source-b/agents");
        fs::create_dir_all(&source_a_agents).unwrap();
        fs::create_dir_all(&source_b_agents).unwrap();
        let orchestrator_path = source_a_agents.join("orchestrator.md");
        let web_a_path = source_a_agents.join("web-researcher.md");
        let web_b_path = source_b_agents.join("web-researcher.md");
        fs::write(
            &orchestrator_path,
            "---\nsubagents:\n- web-researcher\n---\n# Orchestrator\n",
        )
        .unwrap();
        fs::write(&web_a_path, "# Web A").unwrap();
        fs::write(&web_b_path, "# Web B").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/orchestrator.md".into(),
            test_item(
                ItemKind::Agent,
                "orchestrator",
                "source-a",
                orchestrator_path.clone(),
                "agents/orchestrator.md",
            ),
        );
        items.insert(
            "agents/web-researcher__source-a.md".into(),
            test_item(
                ItemKind::Agent,
                "web-researcher__source-a",
                "source-a",
                web_a_path,
                "agents/web-researcher__source-a.md",
            ),
        );
        items.insert(
            "agents/web-researcher__source-b.md".into(),
            test_item(
                ItemKind::Agent,
                "web-researcher__source-b",
                "source-b",
                web_b_path,
                "agents/web-researcher__source-b.md",
            ),
        );

        let mut target = TargetState { items };
        let renames = vec![
            CollisionRename {
                original_name: "web-researcher".into(),
                new_name: "web-researcher__source-a".into(),
                source_name: "source-a".into(),
                kind: ItemKind::Agent,
            },
            CollisionRename {
                original_name: "web-researcher".into(),
                new_name: "web-researcher__source-b".into(),
                source_name: "source-b".into(),
                kind: ItemKind::Agent,
            },
        ];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec![],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_collision_refs(&mut target, &renames, &graph).unwrap();

        let rewritten = target.items["agents/orchestrator.md"]
            .rewritten_content
            .as_ref()
            .expect("agent should have been rewritten");
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        let subagents = match fm.get("subagents").unwrap() {
            serde_yaml::Value::Sequence(seq) => seq
                .iter()
                .filter_map(serde_yaml::Value::as_str)
                .collect::<Vec<_>>(),
            value => panic!("expected subagents sequence, got {value:?}"),
        };
        assert_eq!(subagents, vec!["web-researcher__source-a"]);
    }

    #[test]
    fn collision_rewrites_skill_refs() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("source-a/agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills: [planning]\n---\n# Agent\n").unwrap();

        let skill_a_path = dir.path().join("source-a/skills/planning");
        let skill_b_path = dir.path().join("source-b/skills/planning");
        fs::create_dir_all(&skill_a_path).unwrap();
        fs::create_dir_all(&skill_b_path).unwrap();
        fs::write(skill_a_path.join("SKILL.md"), "# Planning A").unwrap();
        fs::write(skill_b_path.join("SKILL.md"), "# Planning B").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            test_item(
                ItemKind::Agent,
                "coder",
                "source-a",
                agent_path.clone(),
                "agents/coder.md",
            ),
        );
        items.insert(
            "skills/planning__source-a".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-a",
                "source-a",
                skill_a_path,
                "skills/planning__source-a",
            ),
        );
        items.insert(
            "skills/planning__source-b".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-b",
                "source-b",
                skill_b_path,
                "skills/planning__source-b",
            ),
        );

        let mut target = TargetState { items };
        let renames = vec![
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-a".into(),
                source_name: "source-a".into(),
                kind: ItemKind::Skill,
            },
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-b".into(),
                source_name: "source-b".into(),
                kind: ItemKind::Skill,
            },
        ];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec![],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_collision_refs(&mut target, &renames, &graph).unwrap();

        let rewritten = target.items["agents/coder.md"]
            .rewritten_content
            .as_ref()
            .expect("agent should have been rewritten");
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        assert_eq!(fm.skills(), vec!["planning__source-a"]);
    }

    #[test]
    fn explicit_skill_rename_composes_with_collision_rename() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("source-a/agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills: [planning]\n---\n# Agent\n").unwrap();

        let skill_a_path = dir.path().join("source-a/skills/planning");
        let skill_b_path = dir.path().join("source-b/skills/other");
        fs::create_dir_all(&skill_a_path).unwrap();
        fs::create_dir_all(&skill_b_path).unwrap();
        fs::write(skill_a_path.join("SKILL.md"), "# Planning A").unwrap();
        fs::write(skill_b_path.join("SKILL.md"), "# Planning B").unwrap();

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
            "skills/shared__source-a".into(),
            test_item(
                ItemKind::Skill,
                "shared__source-a",
                "source-a",
                skill_a_path,
                "skills/shared__source-a",
            ),
        );
        items.insert(
            "skills/shared__source-b".into(),
            test_item(
                ItemKind::Skill,
                "shared__source-b",
                "source-b",
                skill_b_path,
                "skills/shared__source-b",
            ),
        );

        let mut target = TargetState { items };
        let explicit_renames = vec![ExplicitSkillRename {
            original_name: "planning".into(),
            new_name: "shared".into(),
            source_name: "source-a".into(),
        }];
        let collision_renames = vec![
            CollisionRename {
                original_name: "shared".into(),
                new_name: "shared__source-a".into(),
                source_name: "source-a".into(),
                kind: ItemKind::Skill,
            },
            CollisionRename {
                original_name: "shared".into(),
                new_name: "shared__source-b".into(),
                source_name: "source-b".into(),
                kind: ItemKind::Skill,
            },
        ];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec![],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_skill_refs(&mut target, &explicit_renames, &graph).unwrap();
        rewrite_collision_refs(&mut target, &collision_renames, &graph).unwrap();

        let rewritten = target.items["agents/coder.md"]
            .rewritten_content
            .as_ref()
            .expect("agent should have been rewritten");
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        assert_eq!(fm.skills(), vec!["shared__source-a"]);
    }

    #[test]
    fn collision_rewrites_local_agent_refs_to_dependency() {
        let dir = TempDir::new().unwrap();
        let agent_path = dir.path().join("project/.mars-src/agents/coder.md");
        fs::create_dir_all(agent_path.parent().unwrap()).unwrap();
        fs::write(&agent_path, "---\nskills: [planning]\n---\n# Local Agent\n").unwrap();

        let skill_a_path = dir.path().join("source-a/skills/planning");
        let skill_b_path = dir.path().join("source-b/skills/planning");
        fs::create_dir_all(&skill_a_path).unwrap();
        fs::create_dir_all(&skill_b_path).unwrap();
        fs::write(skill_a_path.join("SKILL.md"), "# Planning A").unwrap();
        fs::write(skill_b_path.join("SKILL.md"), "# Planning B").unwrap();

        let mut items = IndexMap::new();
        items.insert(
            "agents/coder.md".into(),
            test_item(
                ItemKind::Agent,
                "coder",
                "_self",
                agent_path,
                "agents/coder.md",
            ),
        );
        items.insert(
            "skills/planning__source-a".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-a",
                "source-a",
                skill_a_path,
                "skills/planning__source-a",
            ),
        );
        items.insert(
            "skills/planning__source-b".into(),
            test_item(
                ItemKind::Skill,
                "planning__source-b",
                "source-b",
                skill_b_path,
                "skills/planning__source-b",
            ),
        );

        let mut target = TargetState { items };
        let renames = vec![
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-a".into(),
                source_name: "source-a".into(),
                kind: ItemKind::Skill,
            },
            CollisionRename {
                original_name: "planning".into(),
                new_name: "planning__source-b".into(),
                source_name: "source-b".into(),
                kind: ItemKind::Skill,
            },
        ];
        let graph = ResolvedGraph {
            nodes: IndexMap::new(),
            order: vec!["source-a".into(), "source-b".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };

        rewrite_collision_refs(&mut target, &renames, &graph).unwrap();

        let rewritten = target.items["agents/coder.md"]
            .rewritten_content
            .as_ref()
            .expect("local agent should have been rewritten");
        let fm = crate::frontmatter::parse(rewritten).unwrap();
        assert_eq!(fm.skills(), vec!["planning__source-a"]);
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

        let mut target = TargetState { items };
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

        rewrite_collision_refs(&mut target, &renames, &graph).unwrap();

        assert!(target.items["agents/coder.md"].rewritten_content.is_none());
    }
}
