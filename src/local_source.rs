//! Project-local source discovery rooted under `.mars-src/`.
//!
//! Local items intentionally use the same convention walk as dependency packages
//! so nested `.mars-src/**/agents` and `.mars-src/**/skills` layouts behave the
//! same as published source trees.

use std::path::{Path, PathBuf};

use crate::discover::{self, DiscoveredItem};
use crate::error::MarsError;

pub const LOCAL_SOURCE_DIR: &str = ".mars-src";

#[derive(Debug, Clone)]
pub struct LocalDiscoveredItem {
    pub discovered: DiscoveredItem,
    pub root: PathBuf,
}

impl LocalDiscoveredItem {
    pub fn disk_path(&self) -> PathBuf {
        self.root.join(&self.discovered.source_path)
    }
}

pub fn preferred_local_source_root(project_root: &Path) -> PathBuf {
    project_root.join(LOCAL_SOURCE_DIR)
}

pub fn local_discovery_roots(project_root: &Path) -> Vec<PathBuf> {
    vec![preferred_local_source_root(project_root)]
}

pub fn discover_local_items(
    project_root: &Path,
    source_name: Option<&str>,
) -> Result<Vec<LocalDiscoveredItem>, MarsError> {
    let mut merged = Vec::new();

    for root in local_discovery_roots(project_root) {
        let discovered = discover::discover_source(&root, source_name)?;
        for item in discovered {
            merged.push(LocalDiscoveredItem {
                discovered: item,
                root: root.clone(),
            });
        }
    }

    Ok(merged)
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
        let skill_dir =
            preferred_local_source_root(project_root).join("nested/deeper/skills/review");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(agent_dir.join("local.md"), "# local").unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# review").unwrap();

        let items = discover_local_items(project_root, Some("_self")).unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| {
            item.discovered.id.kind == ItemKind::Agent
                && item.discovered.source_path == Path::new("nested/agents/local.md")
        }));
        assert!(items.iter().any(|item| {
            item.discovered.id.kind == ItemKind::Skill
                && item.discovered.source_path == Path::new("nested/deeper/skills/review")
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

        let items = discover_local_items(project_root, Some("_self")).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].discovered.id.kind, ItemKind::Skill);
        assert_eq!(items[0].discovered.id.name.as_str(), "planning");
        assert_eq!(items[0].root, preferred_local_source_root(project_root));
    }
}
