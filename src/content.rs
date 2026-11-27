//! Layered lookup for agent and skill content: the primary root (the project, or the user level
//! outside one), a fallback root, then a host's provided root.
//!
//! The files under `.mars/` are whatever Mars version installed them, not authored package files:
//! every reader parses them with `mars_schema::agent::read` or `skill::read`.
//!
//! The layers are looked up in rank order and the first that claims a name answers. A layer
//! claims a name when its store has the file, or when its lock records collision renames of it:
//! a renamed name never falls through to a later layer's copy. This is `crate::layer`'s claim
//! rule, which sync applies as `layer::yielded`. A layer whose lock does not load is skipped
//! whole, with a warning ([`ContentRoots::warnings`]): its files without its renames would let a
//! renamed name fall through.
//!
//! The primary root remains the sole authority for project configuration. A fallback root
//! contributes only files from its canonical store. A nonexistent or manifest-less fallback is
//! intentionally ignored because callers may pass the user content root before it has been
//! initialized. Harness-specific skill variants resolve beside the selected skill, including
//! when that skill came from the fallback or provided root.
//!
//! A host's content is read only in place. Its copies in a store (records of a `_provided-*`
//! source, and skills `mars provide` recorded in `provided.lock` that no record owns) are
//! invisible here, so a copy an older host installed never outranks the host's own bundle.
//!
//! A provided root is a host's bundle directory, read in place in the layout `mars provide`
//! reads (`agents/<name>.md`, `skills/<name>/SKILL.md`), under its entry rule
//! ([`crate::provided::is_entry_name`]) and refusing what it refuses: a symlink anywhere in the
//! entry. Nothing from it is installed or recorded. A rename is read from the layer's lock,
//! never from the file name: `mars provide`'s `<skill>__<provider>` and a package's own `a__b`
//! look the same on disk.

use crate::layout::Layout;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;
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
    layout: Layout,
    scope: ContentScope,
}

impl ContentRoot {
    pub(crate) fn mars_dir(&self) -> &Path {
        self.layout.store()
    }

    pub(crate) fn scope(&self) -> ContentScope {
        self.scope
    }
}

#[derive(Debug)]
pub(crate) struct ContentRoots {
    /// The installed layers whose lock loaded: the primary root, then the fallback root.
    roots: Vec<InstalledLayer>,
    /// The primary root's store, whether or not its lock loaded.
    primary_store: PathBuf,
    provided_root: Option<PathBuf>,
    /// The installed layers skipped because their lock did not load.
    warnings: Vec<Diagnostic>,
}

#[derive(Debug)]
struct InstalledLayer {
    root: ContentRoot,
    lock: crate::lock::LockFile,
    claims: Claims,
}

/// What a layer's lock says beyond its files: the collision renames of each original name, and
/// the store paths holding a host's copies, which lookup never serves.
#[derive(Debug, Default)]
struct Claims {
    agents: BTreeMap<String, Vec<String>>,
    skills: BTreeMap<String, Vec<String>>,
    /// An agent's `.md` or a skill's directory.
    hidden: HashSet<PathBuf>,
}

/// What a by-name lookup found: an agent's `.md` or a skill's `SKILL.md`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    Found(PathBuf),
    /// The first layer that claims the name has these collision renames of it and no copy.
    Renamed(Vec<String>),
    Missing,
}

impl Lookup {
    pub(crate) fn found(&self) -> Option<&Path> {
        match self {
            Self::Found(path) => Some(path),
            _ => None,
        }
    }

    /// The path, or the error for a `kind` named `name` that no layer has.
    pub(crate) fn require(self, kind: ItemKind, name: &str) -> Result<PathBuf, MarsError> {
        match self {
            Self::Found(path) => Ok(path),
            Self::Renamed(renamed) => Err(not_found(kind, name, &renamed)),
            Self::Missing => Err(not_found(kind, name, &[])),
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
    pub(crate) warnings: Vec<Diagnostic>,
}

/// The error for a `kind` named `name` that no layer has; `renamed` lists the collision
/// renames to use instead, if any.
pub(crate) fn not_found(kind: ItemKind, name: &str, renamed: &[String]) -> MarsError {
    let code = match (kind, renamed.is_empty()) {
        (ItemKind::Agent, true) => "agent_not_found",
        (ItemKind::Agent, false) => "agent_renamed",
        (_, true) => "skill_not_found",
        (_, false) => "skill_renamed",
    };
    MarsError::NotFound {
        code,
        message: not_found_message(kind, name, renamed),
    }
}

/// [`not_found`]'s text, for a caller that reports it as a warning.
pub(crate) fn not_found_message(kind: ItemKind, name: &str, renamed: &[String]) -> String {
    if renamed.is_empty() {
        return format!("{kind} `{name}` not found");
    }
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
        primary: &Layout,
        fallback_root: Option<&Path>,
        provided_root: Option<&Path>,
    ) -> Result<Self, MarsError> {
        let mut layers = vec![ContentRoot {
            layout: primary.clone(),
            scope: ContentScope::Root,
        }];

        if let Some(fallback_root) = fallback_root {
            let fallback = Layout::project(absolute_path(fallback_root));
            if fallback.root() != primary.root() && fallback.manifest().is_file() {
                layers.push(ContentRoot {
                    layout: fallback,
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

        let mut roots = Vec::new();
        let mut warnings = Vec::new();
        for root in layers {
            match crate::lock::load(&root.layout) {
                Ok(lock) => {
                    let claims = Claims::read(&root, &lock);
                    roots.push(InstalledLayer { root, lock, claims });
                }
                Err(err) => warnings.push(Diagnostic::warning(
                    "content-skipped",
                    format!(
                        "skipping the agents and skills installed in {}: {err}",
                        root.layout.root().display()
                    ),
                )),
            }
        }

        Ok(Self {
            roots,
            primary_store: primary.store().to_path_buf(),
            provided_root,
            warnings,
        })
    }

    /// The installed layers skipped because their lock did not load, each a warning the caller
    /// shows with its result.
    pub(crate) fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    pub(crate) fn primary_mars_dir(&self) -> &Path {
        &self.primary_store
    }

    pub(crate) fn agent(&self, agent: &str) -> Lookup {
        self.lookup(ItemKind::Agent, agent)
    }

    pub(crate) fn skill(&self, skill: &str) -> Lookup {
        self.lookup(ItemKind::Skill, skill)
    }

    /// The first layer that claims `name` answers, with its file or its renames of it.
    fn lookup(&self, kind: ItemKind, name: &str) -> Lookup {
        let relative = entry_path(kind, name);
        for layer in &self.roots {
            let path = layer.root.mars_dir().join(&relative);
            let item = match kind {
                ItemKind::Agent => Some(path.as_path()),
                _ => path.parent(),
            };
            if path.is_file() && !item.is_some_and(|item| layer.claims.hidden.contains(item)) {
                return Lookup::Found(path);
            }
            let renames = match kind {
                ItemKind::Agent => &layer.claims.agents,
                _ => &layer.claims.skills,
            };
            if let Some(renamed) = renames.get(name) {
                return Lookup::Renamed(renamed.clone());
            }
        }
        match self.provided_entry(kind, name) {
            Some(path) => Lookup::Found(path),
            None => Lookup::Missing,
        }
    }

    /// The provided root's entry for `name`: an entry name, and a regular agent file or a skill
    /// directory holding only directories and regular files, as `provide` would accept it.
    fn provided_entry(&self, kind: ItemKind, name: &str) -> Option<PathBuf> {
        let root = self.provided_root.as_deref()?;
        if !crate::provided::is_entry_name(name) {
            return None;
        }
        let path = root.join(entry_path(kind, name));
        let is_entry = match kind {
            ItemKind::Agent => path.symlink_metadata().is_ok_and(|meta| meta.is_file()),
            _ => path.is_file() && is_plain_tree(path.parent()?),
        };
        is_entry.then_some(path)
    }

    /// Everything a by-name lookup reaches: each installed layer's locked items, then the
    /// provided root's entries, with [`Self::warnings`].
    pub(crate) fn visible_items(&self, kind: ItemKind) -> Result<VisibleItems, MarsError> {
        let mut items = Vec::new();

        for layer in &self.roots {
            let mars_dir = layer.root.mars_dir();
            for (dest_path, item) in layer.lock.canonical_flat_items() {
                if item.kind != kind {
                    continue;
                }
                let disk_path = dest_path.resolve(mars_dir);
                if !self.is_visible(kind, &disk_path) {
                    continue;
                }
                items.push(VisibleItem {
                    scope: layer.root.scope(),
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

        Ok(VisibleItems {
            items,
            warnings: self.warnings.clone(),
        })
    }

    fn is_visible(&self, kind: ItemKind, disk_path: &Path) -> bool {
        let (name, entry) = match kind {
            ItemKind::Agent => (disk_path.file_stem(), disk_path.to_path_buf()),
            _ => (disk_path.file_name(), disk_path.join("SKILL.md")),
        };
        let Some(name) = name.and_then(|name| name.to_str()) else {
            return false;
        };
        self.lookup(kind, name).found() == Some(entry.as_path())
    }
}

impl Claims {
    /// A layer's collision renames, as sync records them: an item at
    /// `<name><collision_suffix(source)>` in the lock, with at least two in the layer, because
    /// sync renames every dependency on a collision and the consumer's own package keeps the
    /// bare name. And its host copies: a host source's records, and the skills `mars provide`
    /// recorded that no record owns.
    fn read(root: &ContentRoot, lock: &crate::lock::LockFile) -> Self {
        let mut claims = Self::default();
        let mut owned = HashSet::new();
        let (mut agents, mut skills) = (BTreeMap::new(), BTreeMap::new());
        for (dest_path, item) in lock.canonical_flat_items() {
            let disk_path = dest_path.resolve(root.mars_dir());
            if crate::provided::is_provided_source(&item.source) {
                claims.hidden.insert(disk_path);
                continue;
            }
            owned.insert(disk_path);
            let by_name: &mut BTreeMap<String, Vec<String>> = match item.kind {
                ItemKind::Agent => &mut agents,
                ItemKind::Skill => &mut skills,
                _ => continue,
            };
            let renamed = dest_path.item_name(item.kind);
            let suffix = crate::sync::target::collision_suffix(&item.source);
            if let Some(original) = renamed.strip_suffix(&suffix)
                && !original.is_empty()
            {
                by_name
                    .entry(original.to_string())
                    .or_default()
                    .push(renamed);
            }
        }
        for name in crate::provided::recorded_skills(&root.layout) {
            let dir = root.mars_dir().join("skills").join(name);
            if !owned.contains(&dir) {
                claims.hidden.insert(dir);
            }
        }
        for (into, from) in [(&mut claims.agents, agents), (&mut claims.skills, skills)] {
            for (original, mut names) in from.into_iter().filter(|(_, names)| names.len() > 1) {
                names.sort();
                names.dedup();
                into.insert(original, names);
            }
        }
        claims
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

/// Whether `dir` is a real directory holding only directories and regular files: `provide`
/// refuses a skill with a symlink anywhere in it, so the in-place read does too.
fn is_plain_tree(dir: &Path) -> bool {
    // WalkDir follows a symlinked root, so the root is checked on its own.
    dir.symlink_metadata().is_ok_and(|meta| meta.is_dir())
        && walkdir::WalkDir::new(dir).into_iter().all(|entry| {
            entry.is_ok_and(|entry| entry.file_type().is_dir() || entry.file_type().is_file())
        })
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
        if crate::provided::is_entry_name(&name) {
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

        let roots = ContentRoots::new(&Layout::project(&primary), Some(&fallback), None).unwrap();

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

        let roots = ContentRoots::new(&Layout::project(&primary), Some(&fallback), None).unwrap();

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
            let roots =
                ContentRoots::new(&Layout::project(&primary), Some(fallback), None).unwrap();
            assert_eq!(roots.roots.len(), 1);
            assert_eq!(roots.agent("reviewer"), Lookup::Missing);
        }
    }

    fn locked_agent(name: &str, source: &str) -> String {
        format!(
            "[items.\"agent/{name}\"]\nsource = \"{source}\"\nkind = \"agent\"\n\
             source_checksum = \"sha256:a\"\n\
             [[items.\"agent/{name}\".outputs]]\ntarget_root = \".mars\"\n\
             dest_path = \"agents/{name}.md\"\nstate = \"installed\"\n\
             installed_checksum = \"sha256:a\"\n"
        )
    }

    /// A rename is what the lock records, never a `__` in a file name: a renamed name must not
    /// fall through to the host's copy, and `mars provide`'s `<skill>__<provider>` or a
    /// package's own lone `a__b` must not hide it.
    #[test]
    fn only_locked_collision_renames_shadow_provided_content() {
        let temp = TempDir::new().unwrap();
        let primary = temp.path().join("primary");
        let provided = temp.path().join("provided");
        for name in ["dup__pkg-a", "dup__pkg-b", "solo__pkg-a"] {
            write_agent(&primary, name);
        }
        write_skill(&primary, "sessions__meridian");
        fs::write(
            primary.join("mars.lock"),
            format!(
                "version = 3\n{}{}{}",
                locked_agent("dup__pkg-a", "pkg-a"),
                locked_agent("dup__pkg-b", "pkg-b"),
                locked_agent("solo__pkg-a", "pkg-a"),
            ),
        )
        .unwrap();
        let provided_dup = write_agent(&provided, "dup");
        let provided_solo = write_agent(&provided, "solo");
        let provided_sessions = write_skill(&provided, "sessions");
        // The provided root has the bundle layout, not a `.mars` store.
        let provided_root = provided.join(".mars");

        let roots =
            ContentRoots::new(&Layout::project(&primary), None, Some(&provided_root)).unwrap();

        assert_eq!(
            roots.agent("dup"),
            Lookup::Renamed(vec!["dup__pkg-a".into(), "dup__pkg-b".into()])
        );
        assert_eq!(roots.agent("solo"), Lookup::Found(provided_solo));
        assert_eq!(roots.skill("sessions"), Lookup::Found(provided_sessions));
        let listed: Vec<PathBuf> = roots
            .visible_items(ItemKind::Agent)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.disk_path)
            .collect();
        assert!(!listed.contains(&provided_dup));
    }
}
