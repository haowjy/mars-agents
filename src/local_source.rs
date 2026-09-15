//! Self source selection: `.mars-src/` overrides the declared package.
//!
//! Local items intentionally use the same convention walk as dependency packages
//! so nested `.mars-src/**/agents` and `.mars-src/**/skills` layouts follow the
//! same layer-grounding rules as published source trees.

use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::discover::{self, DiscoveredItem};
use crate::error::MarsError;
use crate::types::ItemKind;

pub const LOCAL_SOURCE_DIR: &str = ".mars-src";

#[derive(Debug, Clone)]
pub struct LocalDiscoveredItem {
    pub discovered: DiscoveredItem,
    pub root: PathBuf,
    pub dialect: Dialect,
}

impl LocalDiscoveredItem {
    pub fn disk_path(&self) -> PathBuf {
        self.root.join(&self.discovered.source_path)
    }
}

pub fn preferred_local_source_root(project_root: &Path) -> PathBuf {
    project_root.join(LOCAL_SOURCE_DIR)
}

/// Select self definitions before staging; dependency destination renames happen later.
pub fn discover_local_items(
    project_root: &Path,
    package_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<LocalDiscoveredItem>, MarsError> {
    let root = preferred_local_source_root(project_root);
    let dialect = Dialect::resolve_local(None, &root);
    let mut selected: Vec<_> = discover::discover_source(&root, Some("_self"))?
        .into_iter()
        .map(|discovered| LocalDiscoveredItem {
            discovered,
            root: root.clone(),
            dialect,
        })
        .collect();

    if let Some(name) = package_name {
        for discovered in discover::discover_source(project_root, Some(name))? {
            if !matches!(discovered.id.kind, ItemKind::Agent | ItemKind::Skill) {
                continue;
            }
            if let Some(winner) = selected
                .iter()
                .find(|item| item.discovered.id == discovered.id)
            {
                diag.warn(
                    "local-shadow",
                    format!(
                        "self {} `{}` at `{}` shadows package source `{}`",
                        discovered.id.kind,
                        discovered.id.name,
                        winner.disk_path().display(),
                        project_root.join(&discovered.source_path).display(),
                    ),
                );
                continue;
            }
            selected.push(LocalDiscoveredItem {
                discovered,
                root: project_root.to_path_buf(),
                dialect: Dialect::MarsNative,
            });
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ItemKind;
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
