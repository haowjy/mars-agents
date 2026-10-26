//! Layered lookup for installed agent and skill content.
//!
//! The primary root remains the sole authority for project configuration. A
//! fallback root contributes only files from its canonical `.mars` store. A
//! nonexistent or manifest-less fallback is intentionally ignored because
//! callers may pass the user content root before it has been initialized.
//! Harness-specific skill variants resolve beside the selected skill, including
//! when that skill came from the fallback root.

use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::lock::LockedItem;
use mars_schema::item::ItemKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentScope {
    Root,
    Fallback,
}

impl ContentScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContentRoot {
    project_root: PathBuf,
    scope: ContentScope,
}

impl ContentRoot {
    pub(crate) fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub(crate) fn mars_dir(&self) -> PathBuf {
        self.project_root.join(".mars")
    }

    pub(crate) fn scope(&self) -> ContentScope {
        self.scope
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ContentRoots {
    roots: Vec<ContentRoot>,
}

pub(crate) struct VisibleLockedItems {
    pub(crate) items: Vec<(ContentScope, LockedItem, PathBuf)>,
    pub(crate) warnings: Vec<String>,
}

impl ContentRoots {
    pub(crate) fn new(primary_root: &Path, fallback_root: Option<&Path>) -> Self {
        let primary_root = absolute_path(primary_root);
        let mut roots = vec![ContentRoot {
            project_root: primary_root.clone(),
            scope: ContentScope::Root,
        }];

        if let Some(fallback_root) = fallback_root {
            let fallback_root = absolute_path(fallback_root);
            if fallback_root != primary_root && fallback_root.join("mars.toml").is_file() {
                roots.push(ContentRoot {
                    project_root: fallback_root,
                    scope: ContentScope::Fallback,
                });
            }
        }

        Self { roots }
    }

    pub(crate) fn layers(&self) -> impl Iterator<Item = &ContentRoot> {
        self.roots.iter()
    }

    pub(crate) fn primary_mars_dir(&self) -> PathBuf {
        self.roots[0].mars_dir()
    }

    pub(crate) fn agent_file_path(&self, agent: &str) -> PathBuf {
        self.find_file(Path::new("agents").join(format!("{agent}.md")))
    }

    pub(crate) fn skill_file_path(&self, skill: &str) -> PathBuf {
        self.find_file(Path::new("skills").join(skill).join("SKILL.md"))
    }

    /// Return installed items of `kind` that remain visible after layering.
    ///
    /// The primary lock is authoritative and load failures remain fatal. A
    /// fallback lock is optional state, so an unreadable or corrupt one yields
    /// a warning and causes that entire layer to be skipped.
    pub(crate) fn visible_locked_items(
        &self,
        kind: ItemKind,
    ) -> Result<VisibleLockedItems, MarsError> {
        let mut items = Vec::new();
        let mut warnings = Vec::new();

        for root in self.layers() {
            let lock = match crate::lock::load(root.project_root()) {
                Ok(lock) => lock,
                Err(err) if root.scope() == ContentScope::Fallback => {
                    warnings.push(format!(
                        "skipping fallback content from {}: {err}",
                        root.project_root().display()
                    ));
                    continue;
                }
                Err(err) => return Err(err),
            };
            let mars_dir = root.mars_dir();

            for (dest_path, item) in lock.canonical_flat_items() {
                if item.kind != kind {
                    continue;
                }

                let disk_path = dest_path.resolve(&mars_dir);
                if !self.is_visible(kind, &disk_path) {
                    continue;
                }
                items.push((root.scope(), item, disk_path));
            }
        }

        Ok(VisibleLockedItems { items, warnings })
    }

    fn find_file(&self, relative_to_mars: PathBuf) -> PathBuf {
        let primary = self.roots[0].mars_dir().join(&relative_to_mars);
        if primary.is_file() {
            return primary;
        }

        self.roots
            .get(1)
            .map(|root| root.mars_dir().join(&relative_to_mars))
            .filter(|path| path.is_file())
            .unwrap_or(primary)
    }

    fn is_visible(&self, kind: ItemKind, disk_path: &Path) -> bool {
        match kind {
            ItemKind::Agent => disk_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|name| self.agent_file_path(name) == disk_path),
            ItemKind::Skill => disk_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| self.skill_file_path(name) == disk_path.join("SKILL.md")),
            _ => false,
        }
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = dunce::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write_agent(root: &Path, name: &str) -> PathBuf {
        let path = root.join(".mars").join("agents").join(format!("{name}.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# Agent").unwrap();
        dunce::canonicalize(path).unwrap()
    }

    fn write_skill(root: &Path, name: &str) -> PathBuf {
        let path = root
            .join(".mars")
            .join("skills")
            .join(name)
            .join("SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# Skill").unwrap();
        dunce::canonicalize(path).unwrap()
    }

    fn make_fallback(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("mars.toml"), "").unwrap();
    }

    #[test]
    fn primary_content_shadows_fallback_content() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let fallback = temp.path().join("fallback");
        make_fallback(&fallback);

        let primary_agent = write_agent(&primary, "reviewer");
        write_agent(&fallback, "reviewer");
        let primary_skill = write_skill(&primary, "planning");
        write_skill(&fallback, "planning");

        let roots = ContentRoots::new(&primary, Some(&fallback));

        assert_eq!(roots.agent_file_path("reviewer"), primary_agent);
        assert_eq!(roots.skill_file_path("planning"), primary_skill);
    }

    #[test]
    fn fallback_fills_missing_agent_and_skill() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let fallback = temp.path().join("fallback");
        make_fallback(&fallback);

        let fallback_agent = write_agent(&fallback, "reviewer");
        let fallback_skill = write_skill(&fallback, "planning");

        let roots = ContentRoots::new(&primary, Some(&fallback));

        assert_eq!(roots.agent_file_path("reviewer"), fallback_agent);
        assert_eq!(roots.skill_file_path("planning"), fallback_skill);
    }

    #[test]
    fn absent_or_manifestless_fallback_contributes_nothing() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let absent = temp.path().join("absent");
        let manifestless = temp.path().join("manifestless");
        write_agent(&manifestless, "reviewer");

        for fallback in [&absent, &manifestless] {
            let roots = ContentRoots::new(&primary, Some(fallback));
            assert_eq!(roots.layers().count(), 1);
            assert_eq!(
                roots.agent_file_path("reviewer"),
                primary.join(".mars").join("agents").join("reviewer.md")
            );
        }
    }
}
