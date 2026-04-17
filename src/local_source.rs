use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::discover::{self, DiscoveredItem};
use crate::error::MarsError;
use crate::lock::ItemId;

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

pub fn local_discovery_roots(project_root: &Path, include_legacy_root: bool) -> Vec<PathBuf> {
    let mut roots = vec![preferred_local_source_root(project_root)];
    if include_legacy_root {
        roots.push(project_root.to_path_buf());
    }
    roots
}

pub fn discover_local_items(
    project_root: &Path,
    include_legacy_root: bool,
    source_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<LocalDiscoveredItem>, MarsError> {
    let mut seen: HashMap<ItemId, PathBuf> = HashMap::new();
    let mut merged = Vec::new();

    for root in local_discovery_roots(project_root, include_legacy_root) {
        let discovered = discover::discover_source(&root, source_name)?;
        for item in discovered {
            let current_path = root.join(&item.source_path);
            if let Some(existing_path) = seen.get(&item.id) {
                diag.warn(
                    "duplicate-local-definition",
                    format!(
                        "local {} `{}` is defined in both `{}` and `{}` — using `{}`",
                        item.id.kind,
                        item.id.name,
                        relative_display(project_root, existing_path),
                        relative_display(project_root, &current_path),
                        LOCAL_SOURCE_DIR,
                    ),
                );
                continue;
            }

            seen.insert(item.id.clone(), current_path);
            merged.push(LocalDiscoveredItem {
                discovered: item,
                root: root.clone(),
            });
        }
    }

    Ok(merged)
}
fn relative_display(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ItemKind;
    use tempfile::TempDir;

    #[test]
    fn prefers_mars_src_over_repo_root() {
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

        let mut diag = DiagnosticCollector::new();
        let items = discover_local_items(project_root, true, Some("_self"), &mut diag).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].discovered.id.kind, ItemKind::Skill);
        assert_eq!(items[0].discovered.id.name.as_str(), "planning");
        assert_eq!(items[0].root, preferred_local_source_root(project_root));

        let diagnostics = diag.drain();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "duplicate-local-definition");
        assert!(diagnostics[0].message.contains(".mars-src"));
    }

    #[test]
    fn includes_repo_root_when_preferred_root_is_empty() {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();

        std::fs::create_dir_all(project_root.join("agents")).unwrap();
        std::fs::write(project_root.join("agents").join("coder.md"), "# Coder").unwrap();

        let mut diag = DiagnosticCollector::new();
        let items = discover_local_items(project_root, true, Some("_self"), &mut diag).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].discovered.id.kind, ItemKind::Agent);
        assert_eq!(items[0].discovered.id.name.as_str(), "coder");
        assert_eq!(items[0].root, project_root);
        assert!(diag.is_empty());
    }

    #[test]
    fn skips_legacy_repo_root_without_package_gate() {
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

        let mut diag = DiagnosticCollector::new();
        let items = discover_local_items(project_root, false, Some("_self"), &mut diag).unwrap();

        assert!(items.is_empty());
        assert!(diag.is_empty());
    }
}
