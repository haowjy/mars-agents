//! Self source selection: `.mars-src/` overrides the declared package.
//!
//! Both authored roots are prepared through `mars-schema`; shadow selection is
//! consumer policy applied to those prepared definition lists.

use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::discover::DiscoveredItem;
use crate::error::MarsError;
use mars_schema::item::{DefinitionKind, ItemKind};
use mars_schema::prepare::{PrepareContext, PreparedDefinition};

pub use mars_schema::prepare::LOCAL_SOURCE_DIR;

#[derive(Debug, Clone)]
pub struct LocalDiscoveredItem {
    pub discovered: DiscoveredItem,
    pub root: PathBuf,
    pub(crate) content: LocalItemContent,
}

#[derive(Debug, Clone)]
pub(crate) enum LocalItemContent {
    Prepared {
        definition: Box<PreparedDefinition>,
        payload: Vec<mars_schema::prepare::PayloadEntry>,
    },
    Binary,
    Hook(Box<crate::compiler::hooks::ParsedHookItem>),
    Mcp(Box<crate::compiler::mcp::ParsedMcpItem>),
}

impl LocalDiscoveredItem {
    pub fn disk_path(&self) -> PathBuf {
        self.root.join(&self.discovered.source_path)
    }
}

pub fn preferred_local_source_root(project_root: &Path) -> PathBuf {
    project_root.join(LOCAL_SOURCE_DIR)
}

pub fn discover_local_items(
    project_root: &Path,
    package_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<LocalDiscoveredItem>, MarsError> {
    let local_root = preferred_local_source_root(project_root);
    let mut selected = if local_root.is_dir() {
        let prepared = crate::package_prepare::prepare_authored(
            &local_root,
            PrepareContext::AuthoredLocal,
            None,
            diag,
        )?;
        let mut items = prepared_items(&local_root, Some("_self"), prepared);
        items.extend(binary_items(&local_root, true, diag)?);
        items
    } else {
        Vec::new()
    };

    let mut root_candidates = Vec::new();
    if package_name.is_some() {
        let prepared = crate::package_prepare::prepare_authored(
            project_root,
            PrepareContext::Package,
            None,
            diag,
        )?;
        root_candidates.extend(prepared_items(project_root, package_name, prepared));
    }
    // A package's own `bootstrap/` documents are for its consumers; v0.19.0 selected only the
    // package's Agents and skills (plus the project's hooks and MCP servers) as self items.
    root_candidates.extend(binary_items(project_root, false, diag)?);
    for candidate in root_candidates {
        if let Some(winner) = selected
            .iter()
            .find(|item| item.discovered.id == candidate.discovered.id)
        {
            diag.warn(
                "local-shadow",
                format!(
                    "self {} `{}` at `{}` shadows package source `{}`",
                    candidate.discovered.id.kind,
                    candidate.discovered.id.name,
                    winner.disk_path().display(),
                    candidate.disk_path().display(),
                ),
            );
        } else {
            selected.push(candidate);
        }
    }
    Ok(selected)
}

fn binary_items(
    root: &Path,
    include_bootstrap: bool,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<LocalDiscoveredItem>, MarsError> {
    let mut items = Vec::new();
    if include_bootstrap {
        items.extend(
            crate::discover::discover_bootstrap_surfaces(root, diag)?
                .into_iter()
                .map(|discovered| LocalDiscoveredItem {
                    discovered,
                    root: root.to_path_buf(),
                    content: LocalItemContent::Binary,
                }),
        );
    }
    items.extend(
        crate::compiler::hooks::discover_hook_items(
            root,
            "_self",
            crate::compiler::hooks::HookSource::Project,
            0,
            0,
            diag,
        )?
        .into_iter()
        .map(|hook| LocalDiscoveredItem {
            discovered: hook.discovered_item(root),
            root: root.to_path_buf(),
            content: LocalItemContent::Hook(Box::new(hook)),
        }),
    );
    items.extend(
        crate::compiler::mcp::discover_mcp_items(root, "_self", 0, 0, diag)?
            .into_iter()
            .map(|mcp| {
                let source_path = mcp
                    .source_path
                    .parent()
                    .and_then(|path| path.strip_prefix(root).ok())
                    .expect("local MCP definition belongs to its authored root")
                    .to_path_buf();
                LocalDiscoveredItem {
                    discovered: DiscoveredItem {
                        id: crate::lock::ItemId {
                            kind: ItemKind::McpServer,
                            name: mcp.name.clone().into(),
                        },
                        source_path,
                    },
                    root: root.to_path_buf(),
                    content: LocalItemContent::Mcp(Box::new(mcp)),
                }
            }),
    );
    Ok(items)
}

fn prepared_items(
    root: &Path,
    flat_skill_name: Option<&str>,
    prepared: mars_schema::prepare::Prepared,
) -> Vec<LocalDiscoveredItem> {
    prepared
        .definitions
        .into_iter()
        .map(|mut definition| {
            if definition.source_path == "SKILL.md"
                && definition.slug == "skill"
                && let Some(name) = flat_skill_name
            {
                definition.slug = name.to_owned();
            }
            let kind = ItemKind::from(definition.kind);
            let source_path = match definition.kind {
                DefinitionKind::Agent => PathBuf::from(&definition.source_path),
                DefinitionKind::Skill if definition.source_path == "SKILL.md" => PathBuf::from("."),
                DefinitionKind::Skill => PathBuf::from(&definition.source_path)
                    .parent()
                    .expect("skill definition has a parent")
                    .to_path_buf(),
            };
            let prefix = if source_path == Path::new(".") {
                "".to_string()
            } else {
                format!("{}/", source_path.to_string_lossy().replace('\\', "/"))
            };
            let payload = prepared
                .payload
                .iter()
                .filter(|entry| prefix.is_empty() || entry.source_path.starts_with(&prefix))
                .cloned()
                .collect();
            LocalDiscoveredItem {
                discovered: DiscoveredItem {
                    id: crate::lock::ItemId {
                        kind,
                        name: definition.slug.clone().into(),
                    },
                    source_path,
                },
                root: root.to_path_buf(),
                content: LocalItemContent::Prepared {
                    definition: Box::new(definition),
                    payload,
                },
            }
        })
        .collect()
}

/// Resource exclusions for a flat self skill, relative to its source root.
/// Existing outputs may use absolute paths, dot segments, or symlink aliases.
pub(crate) fn flat_skill_excluded_paths(
    project_root: &Path,
    source_root: &Path,
    targets: &[String],
) -> Result<Vec<PathBuf>, MarsError> {
    let mut excluded: Vec<_> = crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL
        .iter()
        .map(PathBuf::from)
        .collect();
    let source_root = dunce::canonicalize(source_root)?;
    let project_root = dunce::canonicalize(project_root)?;
    if source_root == project_root {
        excluded.extend([PathBuf::from(LOCAL_SOURCE_DIR), PathBuf::from(".agents")]);
        excluded.extend(
            mars_schema::harness::ALL
                .iter()
                .map(|h| PathBuf::from(crate::harness::registry::descriptor(*h).default_target)),
        );
    }
    for target in targets {
        let path = std::path::absolute(project_root.join(target))?;
        if let Ok(relative) = path.strip_prefix(&source_root) {
            excluded.push(relative.to_path_buf());
        }
        if let Ok(real_path) = dunce::canonicalize(&path)
            && let Ok(relative) = real_path.strip_prefix(&source_root)
        {
            excluded.push(relative.to_path_buf());
        }
    }
    Ok(excluded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::item::ItemKind;
    use tempfile::TempDir;

    #[test]
    fn discovers_nested_items_under_mars_src() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();
        let agent_dir = preferred_local_source_root(project_root).join("nested/agents");
        let skill_dir = preferred_local_source_root(project_root).join("nested/skills/review");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(agent_dir.join("local.md"), "# local").unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# review").unwrap();

        let items =
            discover_local_items(project_root, None, &mut DiagnosticCollector::new()).unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.discovered.id.kind == ItemKind::Agent
                && item.discovered.source_path == Path::new("nested/agents/local.md")
        }));
        assert!(items.iter().any(|item| {
            item.discovered.id.kind == ItemKind::Skill
                && item.discovered.source_path == Path::new("nested/skills/review")
        }));
    }
}
