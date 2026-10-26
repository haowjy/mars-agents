//! Self source selection: `.mars-src/` overrides the declared package.
//!
//! Both authored roots are prepared through `mars-schema`; shadow selection is
//! consumer policy applied to those prepared definition lists.

use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::discover::DiscoveredItem;
use crate::error::MarsError;
use mars_schema::item::ItemKind;
use mars_schema::prepare::{PrepareContext, PreparedDefinition};

pub const LOCAL_SOURCE_DIR: &str = ".mars-src";

#[derive(Debug, Clone)]
pub struct LocalDiscoveredItem {
    pub discovered: DiscoveredItem,
    pub root: PathBuf,
    pub(crate) definition: PreparedDefinition,
    pub(crate) payload: Vec<mars_schema::prepare::PayloadEntry>,
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
        prepared_items(&local_root, prepared)
    } else {
        Vec::new()
    };

    if package_name.is_some() {
        let prepared = crate::package_prepare::prepare_authored(
            project_root,
            PrepareContext::Package,
            None,
            diag,
        )?;
        for candidate in prepared_items(project_root, prepared) {
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
    }
    Ok(selected)
}

fn prepared_items(
    root: &Path,
    prepared: mars_schema::prepare::Prepared,
) -> Vec<LocalDiscoveredItem> {
    prepared
        .definitions
        .into_iter()
        .map(|definition| {
            let kind = match definition.kind {
                mars_schema::upgrade::DefinitionKind::Agent => ItemKind::Agent,
                mars_schema::upgrade::DefinitionKind::Skill => ItemKind::Skill,
            };
            let source_path = match kind {
                ItemKind::Agent => PathBuf::from(&definition.source_path),
                ItemKind::Skill if definition.source_path == "SKILL.md" => PathBuf::from("."),
                ItemKind::Skill => PathBuf::from(&definition.source_path)
                    .parent()
                    .expect("skill definition has a parent")
                    .to_path_buf(),
                _ => unreachable!(),
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
                definition,
                payload,
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

    #[test]
    fn discovers_mars_src_not_repo_root() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();

        std::fs::create_dir_all(project_root.join("skills").join("planning")).unwrap();
        std::fs::write(
            project_root
                .join("skills")
                .join("planning")
                .join("SKILL.md"),
            "# Legacy",
        )
        .unwrap();

        let preferred = preferred_local_source_root(project_root)
            .join("skills")
            .join("planning");
        std::fs::create_dir_all(&preferred).unwrap();
        std::fs::write(preferred.join("SKILL.md"), "# Preferred").unwrap();

        let items =
            discover_local_items(project_root, None, &mut DiagnosticCollector::new()).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].discovered.id.kind, ItemKind::Skill);
        assert_eq!(items[0].discovered.id.name.as_str(), "planning");
        assert_eq!(items[0].root, preferred_local_source_root(project_root));
    }
}
