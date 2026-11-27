//! Layered lookup for agent and skill content: the project root, a fallback root, then a host's
//! provided root.
//!
//! The files under `.mars/` are whatever Mars version installed them, not authored package files:
//! every reader parses them with `mars_schema::agent::read` or `skill::read`.
//!
//! The primary root remains the sole authority for project configuration. A
//! fallback root contributes only files from its canonical `.mars` store. A
//! nonexistent or manifest-less fallback is intentionally ignored because
//! callers may pass the user content root before it has been initialized.
//! Harness-specific skill variants resolve beside the selected skill, including
//! when that skill came from the fallback or provided root.
//!
//! A provided root is a host's bundle directory, read in place in the layout `mars provide`
//! reads (`agents/<name>.md`, `skills/<name>/SKILL.md`, `_`/`.`-prefixed names and symlinks are
//! not entries). Nothing from it is installed or recorded. It answers a name only when no
//! installed layer claims it, and a name sync renamed on a collision (`<name>__<source>`) claims
//! it too: falling through would launch the host's copy in place of the user's packages.

use std::path::{Path, PathBuf};

use crate::error::{ConfigError, MarsError};
use mars_schema::item::ItemKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentScope {
    Root,
    Fallback,
    Provided,
}

impl ContentScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Fallback => "fallback",
            Self::Provided => "provided",
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
    /// The installed layers: the project root, then the fallback root.
    roots: Vec<ContentRoot>,
    provided_root: Option<PathBuf>,
}

/// What a by-name lookup found: an agent's `.md` or a skill's `SKILL.md`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    Found(PathBuf),
    /// No layer has the name, and an installed layer has these collision renames of it.
    Renamed(Vec<String>),
    /// No layer has the name; the path it would have in the project root.
    Missing(PathBuf),
}

impl Lookup {
    pub(crate) fn found(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            _ => None,
        }
    }
}

/// An agent or skill visible from the roots, with its source dependency when it came from a
/// lock (`None` for a provided item).
pub(crate) struct VisibleItem {
    pub(crate) scope: ContentScope,
    pub(crate) source: Option<String>,
    /// An agent's `.md` file or a skill's directory.
    pub(crate) disk_path: PathBuf,
}

pub(crate) struct VisibleItems {
    pub(crate) items: Vec<VisibleItem>,
    pub(crate) warnings: Vec<crate::diagnostic::Diagnostic>,
}

/// The message for a name that sync renamed on every side of a collision.
pub(crate) fn renamed_message(kind: ItemKind, name: &str, renamed: &[String]) -> String {
    format!(
        "{kind} `{name}` not found: more than one package defines it, so Mars renamed each: {}. \
         Use one of those names.",
        renamed.join(", ")
    )
}

impl ContentRoots {
    /// Fails when `provided_root` is not a directory: the host named its own bundle, so a
    /// missing one is the host's bug, not an empty layer.
    pub(crate) fn new(
        primary_root: &Path,
        fallback_root: Option<&Path>,
        provided_root: Option<&Path>,
    ) -> Result<Self, MarsError> {
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

        let provided_root = provided_root
            .map(|root| {
                if root.is_dir() {
                    Ok(absolute_path(root))
                } else {
                    Err(ConfigError::Invalid {
                        message: format!("--provided-root {} is not a directory", root.display()),
                    })
                }
            })
            .transpose()?;

        Ok(Self {
            roots,
            provided_root,
        })
    }

    pub(crate) fn layers(&self) -> impl Iterator<Item = &ContentRoot> {
        self.roots.iter()
    }

    pub(crate) fn primary_mars_dir(&self) -> PathBuf {
        self.roots[0].mars_dir()
    }

    pub(crate) fn agent(&self, agent: &str) -> Lookup {
        self.lookup(ItemKind::Agent, agent)
    }

    pub(crate) fn skill(&self, skill: &str) -> Lookup {
        self.lookup(ItemKind::Skill, skill)
    }

    fn lookup(&self, kind: ItemKind, name: &str) -> Lookup {
        let relative = entry_path(kind, name);
        for root in &self.roots {
            let path = root.mars_dir().join(&relative);
            if path.is_file() {
                return Lookup::Found(path);
            }
        }
        let renamed = self.collision_renames(kind, name);
        if !renamed.is_empty() {
            return Lookup::Renamed(renamed);
        }
        if let Some(path) = self.provided_entry(kind, name) {
            return Lookup::Found(path);
        }
        Lookup::Missing(self.roots[0].mars_dir().join(relative))
    }

    /// `<name>__<source>` entries in the installed layers, sorted. Only a lookup that missed
    /// pays for listing the directories.
    fn collision_renames(&self, kind: ItemKind, name: &str) -> Vec<String> {
        let prefix = format!("{name}__");
        let mut renamed: Vec<String> = self
            .roots
            .iter()
            .filter_map(|root| std::fs::read_dir(root.mars_dir().join(kind_dir(kind))).ok())
            .flatten()
            .filter_map(|entry| {
                let file_name = entry.ok()?.file_name().into_string().ok()?;
                let stem = match kind {
                    ItemKind::Agent => file_name.strip_suffix(".md")?.to_string(),
                    _ => file_name,
                };
                stem.starts_with(&prefix).then_some(stem)
            })
            .collect();
        renamed.sort();
        renamed.dedup();
        renamed
    }

    /// The provided root's entry for `name`, under the rules `provide` reads a bundle by.
    fn provided_entry(&self, kind: ItemKind, name: &str) -> Option<PathBuf> {
        let root = self.provided_root.as_deref()?;
        if !is_entry_name(name) {
            return None;
        }
        let path = root.join(entry_path(kind, name));
        let entry = match kind {
            ItemKind::Agent => path.clone(),
            _ => path.parent()?.to_path_buf(),
        };
        let metadata = entry.symlink_metadata().ok()?;
        let is_entry = match kind {
            ItemKind::Agent => metadata.is_file(),
            _ => metadata.is_dir() && path.is_file(),
        };
        is_entry.then_some(path)
    }

    /// Everything a by-name lookup reaches: each installed layer's locked items, then the
    /// provided root's entries. The primary lock is authoritative and load failures remain
    /// fatal. A fallback lock is optional state, so an unreadable or corrupt one yields a
    /// warning and causes that entire layer to be skipped.
    pub(crate) fn visible_items(&self, kind: ItemKind) -> Result<VisibleItems, MarsError> {
        let mut items = Vec::new();
        let mut warnings = Vec::new();

        for root in self.layers() {
            let lock = match crate::lock::load(root.project_root()) {
                Ok(lock) => lock,
                Err(err) if root.scope() == ContentScope::Fallback => {
                    warnings.push(crate::diagnostic::Diagnostic::warning(
                        "content-skipped",
                        format!(
                            "skipping fallback content from {}: {err}",
                            root.project_root().display()
                        ),
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
                items.push(VisibleItem {
                    scope: root.scope(),
                    source: Some(item.source.to_string()),
                    disk_path,
                });
            }
        }

        if let Some(provided_root) = &self.provided_root {
            for name in provided_names(&provided_root.join(kind_dir(kind)), kind)? {
                let disk_path = provided_root.join(kind_dir(kind)).join(match kind {
                    ItemKind::Agent => format!("{name}.md"),
                    _ => name.clone(),
                });
                if self.is_visible(kind, &disk_path) {
                    items.push(VisibleItem {
                        scope: ContentScope::Provided,
                        source: None,
                        disk_path,
                    });
                }
            }
        }

        Ok(VisibleItems { items, warnings })
    }

    fn is_visible(&self, kind: ItemKind, disk_path: &Path) -> bool {
        match kind {
            ItemKind::Agent => disk_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|name| self.agent(name).found() == Some(disk_path)),
            ItemKind::Skill => disk_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    self.skill(name).found() == Some(disk_path.join("SKILL.md").as_path())
                }),
            _ => false,
        }
    }
}

fn kind_dir(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Agent => "agents",
        _ => "skills",
    }
}

fn entry_path(kind: ItemKind, name: &str) -> PathBuf {
    match kind {
        ItemKind::Agent => Path::new("agents").join(format!("{name}.md")),
        _ => Path::new("skills").join(name).join("SKILL.md"),
    }
}

/// `provide`'s rule for a bundle's entries: `_`/`.` names are plumbing (`__pycache__/`), and a
/// name with a separator is not one entry.
fn is_entry_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('_')
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
}

/// The entry names under a provided root's `agents/` or `skills/`, sorted; none when the
/// directory is absent.
fn provided_names(kind_dir: &Path, kind: ItemKind) -> Result<Vec<String>, MarsError> {
    let mut names = Vec::new();
    if !kind_dir.is_dir() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(kind_dir)? {
        let Ok(file_name) = entry?.file_name().into_string() else {
            continue;
        };
        let name = match kind {
            ItemKind::Agent => match file_name.strip_suffix(".md") {
                Some(stem) => stem.to_string(),
                None => continue,
            },
            _ => file_name,
        };
        if is_entry_name(&name) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
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

        let roots = ContentRoots::new(&primary, Some(&fallback), None).unwrap();

        assert_eq!(roots.agent("reviewer"), Lookup::Found(primary_agent));
        assert_eq!(roots.skill("planning"), Lookup::Found(primary_skill));
    }

    #[test]
    fn fallback_fills_missing_agent_and_skill() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let fallback = temp.path().join("fallback");
        make_fallback(&fallback);

        let fallback_agent = write_agent(&fallback, "reviewer");
        let fallback_skill = write_skill(&fallback, "planning");

        let roots = ContentRoots::new(&primary, Some(&fallback), None).unwrap();

        assert_eq!(roots.agent("reviewer"), Lookup::Found(fallback_agent));
        assert_eq!(roots.skill("planning"), Lookup::Found(fallback_skill));
    }

    #[test]
    fn absent_or_manifestless_fallback_contributes_nothing() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let absent = temp.path().join("absent");
        let manifestless = temp.path().join("manifestless");
        write_agent(&manifestless, "reviewer");

        for fallback in [&absent, &manifestless] {
            let roots = ContentRoots::new(&primary, Some(fallback), None).unwrap();
            assert_eq!(roots.layers().count(), 1);
            assert_eq!(
                roots.agent("reviewer"),
                Lookup::Missing(primary.join(".mars").join("agents").join("reviewer.md"))
            );
        }
    }
}
