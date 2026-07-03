use std::collections::HashSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::config::{EffectiveConfig, FilterMode};
use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};
use crate::discover;
use crate::error::MarsError;
use crate::hash;
use crate::lock::{CANONICAL_TARGET_ROOT, ItemId, ItemKind, LockFile, LockIndex};
use crate::resolve::ResolvedGraph;
use crate::sync::filter::apply_filter;
use crate::types::{
    ContentHash, DestPath, ItemName, RenameMap, SourceId, SourceName, SourceOrigin,
};

/// What the `.mars/` canonical store should look like after sync.
///
/// Built from the resolved graph with intent-based filtering applied.
#[derive(Debug, Clone)]
pub struct TargetState {
    /// Keyed by dest_path (relative to `.mars/`).
    pub items: IndexMap<DestPath, TargetItem>,
}

/// A single item in the desired target state.
#[derive(Debug, Clone)]
pub struct TargetItem {
    pub id: ItemId,
    pub source_name: SourceName,
    pub origin: SourceOrigin,
    pub source_id: SourceId,
    /// Path to content in fetched source tree.
    pub source_path: PathBuf,
    /// Relative path under `.mars/` (reflects rename if any).
    pub dest_path: DestPath,
    /// SHA-256 of source content.
    pub source_hash: ContentHash,
    /// True when this item comes from root-level `SKILL.md` flat skill discovery.
    pub is_flat_skill: bool,
    /// Optional in-memory content override after frontmatter rewrites.
    pub rewritten_content: Option<String>,
}

/// Explicit skill rename that changes the installed skill name.
#[derive(Debug, Clone)]
pub struct ExplicitSkillRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
}

/// Automatic rename applied when multiple sources target the same destination.
#[derive(Debug, Clone)]
pub struct CollisionRename {
    pub original_name: ItemName,
    pub new_name: ItemName,
    pub source_name: SourceName,
    pub kind: ItemKind,
}

/// Build target state with collision detection integrated.
///
/// This is the main entry point — it builds the target, applies explicit
/// rename mappings, and auto-renames cross-source agent/skill destination
/// collisions.
pub fn build_with_collisions(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
) -> Result<(TargetState, Vec<ExplicitSkillRename>, Vec<CollisionRename>), MarsError> {
    let mut diag = DiagnosticCollector::new();
    build_with_collisions_and_diag(graph, config, &mut diag)
}

pub fn build_with_collisions_and_diag(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
    diag: &mut DiagnosticCollector,
) -> Result<(TargetState, Vec<ExplicitSkillRename>, Vec<CollisionRename>), MarsError> {
    let mut collected_items = Vec::new();
    let mut explicit_skill_renames = Vec::new();

    for source_name in &graph.order {
        let node = &graph.nodes[source_name];
        let source_config = config.dependencies.get(source_name);

        let discovered = discover::discover_resolved_source(
            &node.rooted_ref.package_root,
            Some(source_name.as_str()),
        )?;

        let source_id = source_config
            .map(|s| s.id.clone())
            .unwrap_or_else(|| node.source_id.clone());

        let Some(filters) = graph
            .filters
            .get(source_name)
            .filter(|filters| !filters.is_empty())
            .cloned()
            .or_else(|| source_config.map(|source| vec![source.filter.clone()]))
        else {
            // No materialization request reached this transitive source.
            continue;
        };

        let renames = source_config
            .map(|s| &s.rename)
            .cloned()
            .unwrap_or_default();

        let filtered = apply_filter_union(&discovered, &filters, &node.rooted_ref.package_root)?;

        for item in filtered {
            let is_flat_skill =
                item.id.kind == ItemKind::Skill && item.source_path == Path::new(".");
            let source_content_path = node.rooted_ref.package_root.join(&item.source_path);
            let source_hash = if is_flat_skill {
                ContentHash::from(hash::compute_skill_hash_filtered(
                    &source_content_path,
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
                )?)
            } else {
                ContentHash::from(hash::compute_hash(&source_content_path, item.id.kind)?)
            };

            let (dest_name, dest_path) =
                apply_item_rename(item.id.kind, &item.id.name, &renames, source_name)?;
            if item.id.kind == ItemKind::Agent
                && let Err(message) = crate::target::validate_agent_filename(dest_name.as_str())
            {
                diag.error_with_category(
                    "invalid-agent-filename",
                    format!("{message}; skipping agent from source `{source_name}`"),
                    DiagnosticCategory::Validation,
                );
                continue;
            }
            if item.id.kind == ItemKind::Skill && dest_name != item.id.name {
                explicit_skill_renames.push(ExplicitSkillRename {
                    original_name: item.id.name.clone(),
                    new_name: dest_name.clone(),
                    source_name: source_name.clone(),
                });
            }

            let target_item = TargetItem {
                id: ItemId {
                    kind: item.id.kind,
                    name: dest_name,
                },
                source_name: source_name.clone(),
                origin: SourceOrigin::Dependency(source_name.clone()),
                source_id: source_id.clone(),
                source_path: source_content_path,
                dest_path,
                source_hash,
                is_flat_skill,
                rewritten_content: None,
            };

            collected_items.push(target_item);
        }
    }

    let collision_renames = rename_destination_collisions(&mut collected_items, diag)?;

    let mut items: IndexMap<DestPath, TargetItem> = IndexMap::new();
    for target_item in collected_items {
        if let Some(existing) = items.get(&target_item.dest_path) {
            return Err(MarsError::Collision {
                item: format!(
                    "{} `{}` at `{}` after auto-rename",
                    target_item.id.kind, target_item.id.name, target_item.dest_path
                ),
                source_a: existing.source_name.to_string(),
                source_b: target_item.source_name.to_string(),
            });
        }

        items.insert(target_item.dest_path.clone(), target_item);
    }

    Ok((
        TargetState { items },
        explicit_skill_renames,
        collision_renames,
    ))
}

fn rename_destination_collisions(
    items: &mut [TargetItem],
    diag: &mut DiagnosticCollector,
) -> Result<Vec<CollisionRename>, MarsError> {
    let mut groups: IndexMap<DestPath, Vec<usize>> = IndexMap::new();
    for (index, item) in items.iter().enumerate() {
        groups
            .entry(item.dest_path.clone())
            .or_default()
            .push(index);
    }

    let mut renames = Vec::new();
    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let first = &items[indices[0]];
        let distinct_sources: HashSet<&SourceName> = indices
            .iter()
            .map(|&index| &items[index].source_name)
            .collect();
        let auto_renamable = matches!(first.id.kind, ItemKind::Agent | ItemKind::Skill)
            && indices
                .iter()
                .all(|&index| items[index].id.kind == first.id.kind)
            && distinct_sources.len() == indices.len();
        if !auto_renamable {
            let second = &items[indices[1]];
            return Err(MarsError::Collision {
                item: format!("{} `{}`", second.id.kind, second.id.name),
                source_a: first.source_name.to_string(),
                source_b: second.source_name.to_string(),
            });
        }

        for &index in indices {
            let item = &mut items[index];
            let original_name = item.id.name.clone();
            let new_dest_path =
                suffixed_collision_dest_path(&item.dest_path, item.id.kind, &item.source_name)?;
            let new_name = ItemName::from(dest_name_from_dest(&new_dest_path, item.id.kind));

            diag.warn(
                "auto-rename-collision",
                format!(
                    "auto-renamed {} `{}` from source `{}` → `{}`",
                    item.id.kind, original_name, item.source_name, new_name
                ),
            );

            item.id.name = new_name.clone();
            item.dest_path = new_dest_path;
            renames.push(CollisionRename {
                original_name,
                new_name,
                source_name: item.source_name.clone(),
                kind: item.id.kind,
            });
        }
    }

    Ok(renames)
}

fn apply_filter_union(
    discovered: &[discover::DiscoveredItem],
    filters: &[FilterMode],
    package_root: &Path,
) -> Result<Vec<discover::DiscoveredItem>, MarsError> {
    if filters.is_empty() {
        return Ok(discovered.to_vec());
    }

    let mut union: HashSet<(ItemKind, ItemName, PathBuf)> = HashSet::new();
    for filter in filters {
        let filtered = apply_filter(discovered, filter, package_root)?;
        union.extend(
            filtered
                .iter()
                .map(|item| (item.id.kind, item.id.name.clone(), item.source_path.clone())),
        );
    }

    Ok(discovered
        .iter()
        .filter(|item| {
            union.contains(&(item.id.kind, item.id.name.clone(), item.source_path.clone()))
        })
        .cloned()
        .collect())
}

// Re-export for API compatibility — rewrite helpers live in sync::rewrite.
pub use crate::sync::rewrite::{rewrite_collision_refs, rewrite_skill_refs};

/// Existing on-disk destination that is not lock-managed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmanagedCollision {
    pub source_name: SourceName,
    pub path: DestPath,
}

/// Detect target installs that would overwrite unmanaged on-disk content.
///
/// If a target destination already exists but is not tracked in the lock file,
/// treat it as user-authored content and report it as a collision so callers can
/// skip installation while leaving existing files untouched.
pub fn check_unmanaged_collisions(
    install_target: &Path,
    lock: &LockFile,
    target: &TargetState,
    force: bool,
) -> Vec<UnmanagedCollision> {
    let mut collisions = Vec::new();
    let lock_index = LockIndex::new(lock);

    for (dest_key, target_item) in &target.items {
        if lock_index.contains_output(CANONICAL_TARGET_ROOT, dest_key) {
            continue;
        }

        let disk_path = target_item.dest_path.resolve(install_target);
        if disk_path.exists() {
            if force {
                continue;
            }
            // Check if disk content matches what we'd install — if so,
            // this is a partial prior install (crash recovery), not an
            // unmanaged user file. Safe to overwrite.
            let hash_path = hash_path_for_kind(&disk_path, target_item.id.kind);
            if let Ok(disk_hash) = hash::compute_hash(&hash_path, target_item.id.kind)
                && disk_hash == target_item.source_hash.as_str()
            {
                continue;
            }

            collisions.push(UnmanagedCollision {
                source_name: target_item.source_name.clone(),
                path: target_item.dest_path.clone(),
            });
        }
    }

    collisions
}

fn apply_item_rename(
    kind: ItemKind,
    item_name: &str,
    renames: &RenameMap,
    source_name: &SourceName,
) -> Result<(ItemName, DestPath), MarsError> {
    let default_dest = default_dest_path(kind, item_name);
    let default_key = default_dest.as_str();

    let rename_value = renames.get(default_key).or_else(|| renames.get(item_name));

    let dest_path = match rename_value {
        Some(value) => parse_rename_dest(kind, value.as_str(), source_name)?,
        None => default_dest,
    };
    let dest_name = dest_name_from_dest(&dest_path, kind);

    Ok((ItemName::from(dest_name), dest_path))
}

/// Construct the default destination path for an item.
/// Uses string formatting to guarantee forward slashes on all platforms.
fn default_dest_path(kind: ItemKind, name: &str) -> DestPath {
    let path_str = match kind {
        ItemKind::Agent => format!("agents/{name}.md"),
        ItemKind::Skill => format!("skills/{name}"),
        ItemKind::Hook => format!("hooks/{name}"),
        ItemKind::McpServer => format!("mcp/{name}"),
        ItemKind::BootstrapDoc => format!("bootstrap/{name}/BOOTSTRAP.md"),
    };
    // Safe: internal paths constructed from validated item names
    DestPath::new(path_str).expect("internal default path is always valid")
}

fn parse_rename_dest(
    kind: ItemKind,
    rename_value: &str,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    // Normalize backslashes to forward slashes for cross-platform handling
    let normalized = rename_value.replace('\\', "/");
    let has_prefix = normalized.starts_with("agents/")
        || normalized.starts_with("skills/")
        || normalized.starts_with("hooks/")
        || normalized.starts_with("mcp/")
        || normalized.starts_with("bootstrap/");
    let has_parent = normalized.contains('/');

    if has_prefix || has_parent {
        let dest = if kind == ItemKind::BootstrapDoc && !normalized.ends_with("/BOOTSTRAP.md") {
            format!("{normalized}/BOOTSTRAP.md")
        } else {
            normalized.clone()
        };
        return DestPath::new(&dest).map_err(|e| MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("invalid rename destination `{rename_value}`: {e}"),
        });
    }

    let path_str = match kind {
        ItemKind::Agent => {
            if normalized.ends_with(".md") {
                format!("agents/{normalized}")
            } else {
                format!("agents/{normalized}.md")
            }
        }
        ItemKind::Skill => format!("skills/{normalized}"),
        ItemKind::Hook => format!("hooks/{normalized}"),
        ItemKind::McpServer => format!("mcp/{normalized}"),
        ItemKind::BootstrapDoc => format!("bootstrap/{normalized}/BOOTSTRAP.md"),
    };
    DestPath::new(path_str).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid rename destination `{rename_value}`: {e}"),
    })
}

fn dest_name_from_dest(dest_path: &DestPath, kind: ItemKind) -> String {
    match kind {
        ItemKind::BootstrapDoc => dest_path.item_name(kind),
        _ => {
            let last = dest_path.as_str().rsplit('/').next().unwrap_or("");
            match kind {
                ItemKind::Agent => last.strip_suffix(".md").unwrap_or(last).to_string(),
                ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => last.to_string(),
                ItemKind::BootstrapDoc => unreachable!("handled above"),
            }
        }
    }
}

fn suffixed_collision_dest_path(
    dest_path: &DestPath,
    kind: ItemKind,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let suffix = format!("__{source_name}");
    let path = dest_path.as_str();
    let renamed = match kind {
        ItemKind::Agent => {
            let (parent, leaf) = split_parent_leaf(path);
            let stem = leaf.strip_suffix(".md").unwrap_or(leaf);
            join_parent_leaf(parent, &format!("{stem}{suffix}.md"))
        }
        ItemKind::Skill => {
            return suffixed_leaf_dest_path(path, &suffix, source_name);
        }
        ItemKind::Hook | ItemKind::McpServer | ItemKind::BootstrapDoc => {
            unreachable!("only agent and skill collisions are auto-renamed")
        }
    };

    DestPath::new(&renamed).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid auto-renamed destination `{renamed}`: {e}"),
    })
}

fn suffixed_leaf_dest_path(
    path: &str,
    suffix: &str,
    source_name: &SourceName,
) -> Result<DestPath, MarsError> {
    let (parent, leaf) = split_parent_leaf(path);
    let renamed = join_parent_leaf(parent, &format!("{leaf}{suffix}"));
    DestPath::new(&renamed).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("invalid auto-renamed destination `{renamed}`: {e}"),
    })
}

fn split_parent_leaf(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn join_parent_leaf(parent: &str, leaf: &str) -> String {
    if parent.is_empty() {
        leaf.to_string()
    } else {
        format!("{parent}/{leaf}")
    }
}

fn hash_path_for_kind(path: &Path, kind: ItemKind) -> PathBuf {
    if kind == ItemKind::BootstrapDoc {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::lock::LockFile;
    use crate::resolve::{ResolvedGraph, ResolvedNode};
    use crate::source::ResolvedRef;
    use indexmap::IndexMap;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: create a source tree with agents and skills
    fn make_source_tree(agents: &[(&str, &str)], skills: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        if !agents.is_empty() {
            let agents_dir = dir.path().join("agents");
            fs::create_dir_all(&agents_dir).unwrap();
            for (name, content) in agents {
                fs::write(agents_dir.join(name), content).unwrap();
            }
        }
        if !skills.is_empty() {
            let skills_dir = dir.path().join("skills");
            fs::create_dir_all(&skills_dir).unwrap();
            for (name, content) in skills {
                let skill_dir = skills_dir.join(name);
                fs::create_dir_all(&skill_dir).unwrap();
                fs::write(skill_dir.join("SKILL.md"), content).unwrap();
            }
        }
        dir
    }

    fn make_graph_and_config(
        sources: Vec<(&str, &TempDir, Option<&str>, FilterMode)>,
    ) -> (ResolvedGraph, EffectiveConfig) {
        let mut nodes = IndexMap::new();
        let mut order = Vec::new();
        let mut config_dependencies = IndexMap::new();

        for (name, tree, url, filter) in sources {
            let url_str = url.map(|u| u.to_string());
            nodes.insert(
                name.into(),
                ResolvedNode {
                    source_name: name.into(),
                    source_id: if let Some(u) = url {
                        SourceId::git(crate::types::SourceUrl::from(u))
                    } else {
                        SourceId::Path {
                            canonical: tree.path().to_path_buf(),
                            subpath: None,
                        }
                    },
                    rooted_ref: crate::resolve::RootedSourceRef {
                        checkout_root: tree.path().to_path_buf(),
                        package_root: tree.path().to_path_buf(),
                    },
                    resolved_ref: ResolvedRef {
                        source_name: name.into(),
                        version: None,
                        version_tag: None,
                        commit: None,
                        tree_path: tree.path().to_path_buf(),
                    },
                    latest_version: None,
                    manifest: None,
                    deps: vec![],
                },
            );
            order.push(name.into());

            let spec = if let Some(u) = url {
                SourceSpec::Git(GitSpec {
                    url: crate::types::SourceUrl::from(u),
                    version: None,
                })
            } else {
                SourceSpec::Path(tree.path().to_path_buf())
            };

            config_dependencies.insert(
                name.into(),
                EffectiveDependency {
                    name: name.into(),
                    id: if let Some(u) = url {
                        SourceId::git(crate::types::SourceUrl::from(u))
                    } else {
                        SourceId::Path {
                            canonical: tree.path().to_path_buf(),
                            subpath: None,
                        }
                    },
                    spec,
                    subpath: None,
                    filter,
                    rename: RenameMap::new(),
                    dialect: None,
                    is_overridden: false,
                    original_git: url_str.map(|u| GitSpec {
                        url: crate::types::SourceUrl::from(u),
                        version: None,
                    }),
                },
            );
        }

        let graph = ResolvedGraph {
            nodes,
            order,
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let config = EffectiveConfig {
            dependencies: config_dependencies,
            settings: Settings::default(),
            skills: indexmap::IndexMap::new(),
        };
        (graph, config)
    }

    // === Target build tests ===

    #[test]
    fn build_single_source_no_filter() {
        let tree = make_source_tree(&[("coder.md", "# coder")], &[("planning", "# planning")]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2);
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(target.items.contains_key("skills/planning"));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn invalid_windows_agent_filename_emits_diagnostic_and_skips() {
        // This test creates a file with `:` in the name, which is only possible on
        // non-Windows. The validation catches names that would break on Windows when
        // created on POSIX systems.
        let tree = make_source_tree(&[("bad:name.md", "# bad"), ("coder.md", "# coder")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);
        let mut diag = DiagnosticCollector::new();

        let (target, _, _) = build_with_collisions_and_diag(&graph, &config, &mut diag).unwrap();
        let diagnostics = diag.drain();

        assert!(!target.items.contains_key("agents/bad:name.md"));
        assert!(target.items.contains_key("agents/coder.md"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "invalid-agent-filename");
    }

    #[test]
    fn build_with_path_rename_mapping() {
        let tree = make_source_tree(&[("old-name.md", "# old")], &[]);

        let (graph, mut config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        // Add rename mapping
        config
            .dependencies
            .get_mut("base")
            .unwrap()
            .rename
            .insert("agents/old-name.md".into(), "agents/new-name.md".into());

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 1);
        assert!(target.items.contains_key("agents/new-name.md"));
        assert_eq!(target.items["agents/new-name.md"].id.name, "new-name");
    }

    #[test]
    fn default_dest_path_uses_forward_slashes_for_agents_and_skills() {
        let agent = default_dest_path(ItemKind::Agent, "coder");
        let skill = default_dest_path(ItemKind::Skill, "planning");

        assert_eq!(agent.as_str(), "agents/coder.md");
        assert_eq!(skill.as_str(), "skills/planning");
        assert!(!agent.as_str().contains('\\'));
        assert!(!skill.as_str().contains('\\'));
    }

    #[test]
    fn parse_rename_dest_normalizes_backslashes_to_forward_slashes() {
        let source_name = SourceName::from("base");

        let agent =
            parse_rename_dest(ItemKind::Agent, r"agents\nested\renamed.md", &source_name).unwrap();
        let skill =
            parse_rename_dest(ItemKind::Skill, r"skills\nested\planning", &source_name).unwrap();

        assert_eq!(agent.as_str(), "agents/nested/renamed.md");
        assert_eq!(skill.as_str(), "skills/nested/planning");
        assert!(!agent.as_str().contains('\\'));
        assert!(!skill.as_str().contains('\\'));
    }

    #[test]
    fn parse_rename_dest_rejects_absolute_and_escape_destinations() {
        let source_name = SourceName::from("base");

        let absolute = parse_rename_dest(ItemKind::Agent, "/tmp/escape", &source_name)
            .expect_err("absolute rename should fail");
        assert!(matches!(absolute, MarsError::Source { .. }));

        let traversal = parse_rename_dest(ItemKind::Skill, "../escape", &source_name)
            .expect_err("traversal rename should fail");
        assert!(matches!(traversal, MarsError::Source { .. }));
    }

    #[test]
    fn build_with_invalid_rename_destination_returns_error() {
        let tree = make_source_tree(&[("old-name.md", "# old")], &[]);

        let (graph, mut config) =
            make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);

        config
            .dependencies
            .get_mut("base")
            .unwrap()
            .rename
            .insert("agents/old-name.md".into(), "../escape.md".into());

        let err = build_with_collisions(&graph, &config).unwrap_err();
        assert!(matches!(err, MarsError::Source { .. }));
    }

    // === Collision tests ===

    #[test]
    fn collision_auto_renames_both() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            (
                "source-a",
                &tree1,
                Some("https://github.com/alice/agents"),
                FilterMode::All,
            ),
            (
                "source-b",
                &tree2,
                Some("https://github.com/bob/agents"),
                FilterMode::All,
            ),
        ]);
        let mut diag = DiagnosticCollector::new();

        let (target, explicit_renames, collision_renames) =
            build_with_collisions_and_diag(&graph, &config, &mut diag).unwrap();
        let diagnostics = diag.drain();

        assert!(explicit_renames.is_empty());
        assert_eq!(collision_renames.len(), 2);
        assert!(target.items.contains_key("agents/coder__source-a.md"));
        assert!(target.items.contains_key("agents/coder__source-b.md"));
        assert!(!target.items.contains_key("agents/coder.md"));
        assert_eq!(
            target.items["agents/coder__source-a.md"].id.name,
            "coder__source-a"
        );
        assert_eq!(
            target.items["agents/coder__source-b.md"].id.name,
            "coder__source-b"
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "auto-rename-collision")
                .count(),
            2
        );
    }

    #[test]
    fn skill_collision_auto_renames_both() {
        let tree1 = make_source_tree(&[], &[("planning", "# planning from source 1")]);
        let tree2 = make_source_tree(&[], &[("planning", "# planning from source 2")]);

        let (graph, config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);

        let (target, explicit_renames, collision_renames) =
            build_with_collisions(&graph, &config).unwrap();

        assert!(explicit_renames.is_empty());
        assert_eq!(collision_renames.len(), 2);
        assert!(target.items.contains_key("skills/planning__source-a"));
        assert!(target.items.contains_key("skills/planning__source-b"));
        assert!(!target.items.contains_key("skills/planning"));
        assert_eq!(
            target.items["skills/planning__source-a"].id.name,
            "planning__source-a"
        );
        assert_eq!(
            target.items["skills/planning__source-b"].id.name,
            "planning__source-b"
        );
    }

    #[test]
    fn three_way_collision_renames_all() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);
        let tree3 = make_source_tree(&[("coder.md", "# coder from source 3")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
            ("source-c", &tree3, None, FilterMode::All),
        ]);

        let (target, _, collision_renames) = build_with_collisions(&graph, &config).unwrap();

        assert_eq!(collision_renames.len(), 3);
        assert!(target.items.contains_key("agents/coder__source-a.md"));
        assert!(target.items.contains_key("agents/coder__source-b.md"));
        assert!(target.items.contains_key("agents/coder__source-c.md"));
        assert!(!target.items.contains_key("agents/coder.md"));
    }

    #[test]
    fn explicit_rename_prevents_collision() {
        let tree1 = make_source_tree(&[("coder.md", "# coder from source 1")], &[]);
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);

        let (graph, mut config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/coder.md".into(), "agents/source-a-coder.md".into());

        let (target, _, collision_renames) = build_with_collisions(&graph, &config).unwrap();

        assert!(collision_renames.is_empty());
        assert!(target.items.contains_key("agents/source-a-coder.md"));
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(!target.items.contains_key("agents/coder__source-a.md"));
        assert!(!target.items.contains_key("agents/coder__source-b.md"));
    }

    #[test]
    fn same_source_explicit_rename_collision_stays_hard_error() {
        let tree = make_source_tree(
            &[
                ("coder.md", "# coder"),
                ("reviewer.md", "# reviewer renamed into coder"),
            ],
            &[],
        );
        let (graph, mut config) =
            make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/reviewer.md".into(), "agents/coder.md".into());

        let err = build_with_collisions(&graph, &config).unwrap_err();

        assert!(matches!(err, MarsError::Collision { .. }));
    }

    #[test]
    fn mixed_kind_explicit_rename_collision_stays_hard_error() {
        let tree = make_source_tree(&[("coder.md", "# coder")], &[("planning", "# planning")]);
        let (graph, mut config) =
            make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/coder.md".into(), "skills/planning".into());

        let err = build_with_collisions(&graph, &config).unwrap_err();

        assert!(matches!(err, MarsError::Collision { .. }));
    }

    #[test]
    fn duplicate_source_in_cross_source_group_stays_hard_error_without_auto_warning() {
        let tree1 = make_source_tree(
            &[
                ("coder.md", "# coder"),
                ("reviewer.md", "# reviewer renamed into coder"),
            ],
            &[],
        );
        let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);
        let (graph, mut config) = make_graph_and_config(vec![
            ("source-a", &tree1, None, FilterMode::All),
            ("source-b", &tree2, None, FilterMode::All),
        ]);
        config
            .dependencies
            .get_mut("source-a")
            .unwrap()
            .rename
            .insert("agents/reviewer.md".into(), "agents/coder.md".into());
        let mut diag = DiagnosticCollector::new();

        let err = build_with_collisions_and_diag(&graph, &config, &mut diag).unwrap_err();
        let diagnostics = diag.drain();

        assert!(matches!(err, MarsError::Collision { .. }));
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "auto-rename-collision")
        );
    }

    #[test]
    fn no_collision_no_renames() {
        let tree1 = make_source_tree(&[("coder.md", "# coder")], &[]);
        let tree2 = make_source_tree(&[("reviewer.md", "# reviewer")], &[]);

        let (graph, config) = make_graph_and_config(vec![
            (
                "source-a",
                &tree1,
                Some("https://github.com/alice/agents"),
                FilterMode::All,
            ),
            (
                "source-b",
                &tree2,
                Some("https://github.com/bob/agents"),
                FilterMode::All,
            ),
        ]);

        let (target, renames, collision_renames) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert!(collision_renames.is_empty());
        assert_eq!(target.items.len(), 2);
    }

    // === Source with agents filter + skill deps ===

    #[test]
    fn build_with_agents_filter_pulls_transitive_skills() {
        let tree = make_source_tree(
            &[("coder.md", "---\nskills:\n  - planning\n---\n# Coder\n")],
            &[("planning", "# Planning"), ("unused-skill", "# Unused")],
        );

        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            None,
            FilterMode::Include {
                agents: vec!["coder".into()],
                skills: vec![],
            },
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2); // coder + planning
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(target.items.contains_key("skills/planning"));
        // unused-skill should NOT be present
        assert!(!target.items.contains_key("skills/unused-skill"));
    }

    #[test]
    fn build_with_exclude_filter() {
        let tree = make_source_tree(&[("coder.md", "# coder"), ("deprecated.md", "# old")], &[]);

        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            None,
            FilterMode::Exclude(vec!["deprecated".into()]),
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 1);
        assert!(target.items.contains_key("agents/coder.md"));
    }

    #[test]
    fn build_unions_multiple_include_filters_for_same_source() {
        let tree = make_source_tree(
            &[],
            &[
                ("skill-a", "# Skill A"),
                ("skill-b", "# Skill B"),
                ("skill-c", "# Skill C"),
            ],
        );

        let (mut graph, config) =
            make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);
        graph.filters.insert(
            "base".into(),
            vec![
                FilterMode::Include {
                    agents: vec![],
                    skills: vec!["skill-a".into(), "skill-b".into()],
                },
                FilterMode::Include {
                    agents: vec![],
                    skills: vec!["skill-b".into(), "skill-c".into()],
                },
            ],
        );

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 3);
        assert!(target.items.contains_key("skills/skill-a"));
        assert!(target.items.contains_key("skills/skill-b"));
        assert!(target.items.contains_key("skills/skill-c"));
    }

    #[test]
    fn build_target_items_have_correct_hashes() {
        let content = "# agent content for hash test";
        let tree = make_source_tree(&[("test.md", content)], &[]);

        let (graph, config) = make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        let item = &target.items["agents/test.md"];
        let expected_hash = hash::hash_bytes(content.as_bytes());
        assert_eq!(item.source_hash, expected_hash);
    }

    #[test]
    fn unmanaged_disk_path_collision_reported() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // Existing user-authored file at the same destination.
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# user-authored").unwrap();

        let collisions =
            check_unmanaged_collisions(install_root.path(), &LockFile::empty(), &target, false);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].source_name.as_ref(), "base");
        assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
    }

    #[test]
    fn unmanaged_collision_skipped_when_hash_matches() {
        let content = "# managed agent";
        let tree = make_source_tree(&[("coder.md", content)], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // Simulate partial prior install: file on disk with same content
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, content).unwrap();

        // Should skip collision — disk content matches planned install (crash recovery)
        let collisions =
            check_unmanaged_collisions(install_root.path(), &LockFile::empty(), &target, false);
        assert!(collisions.is_empty());
    }

    #[test]
    fn unmanaged_collision_reported_on_different_content() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        // User-authored file with different content
        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# different user content").unwrap();

        let collisions =
            check_unmanaged_collisions(install_root.path(), &LockFile::empty(), &target, false);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].source_name.as_ref(), "base");
        assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
    }

    #[test]
    fn unmanaged_collision_skipped_under_force() {
        let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
        let (graph, config) = make_graph_and_config(vec![(
            "base",
            &tree,
            Some("https://github.com/org/base"),
            FilterMode::All,
        )]);

        let (target, renames, _) = build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        let install_root = TempDir::new().unwrap();

        let existing = install_root.path().join("agents").join("coder.md");
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "# stale cache content").unwrap();

        let collisions =
            check_unmanaged_collisions(install_root.path(), &LockFile::empty(), &target, true);
        assert!(collisions.is_empty());
    }
}
