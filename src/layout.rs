//! Where a Mars root keeps its files: the one place that turns a root into the paths of its
//! manifest, locks, store, local source and targets. Code that needs one of these paths asks the
//! layout; it never joins a file name onto the root itself.

use std::path::{Path, PathBuf};

use crate::lock::CANONICAL_TARGET_ROOT;

/// The file names diagnostics use for a project's config files.
pub(crate) const CONFIG_FILE: &str = "mars.toml";
pub(crate) const LOCAL_CONFIG_FILE: &str = "mars.local.toml";
pub(crate) const LOCK_FILE: &str = "mars.lock";

/// What a root is. A project is a directory with `mars.toml`; the user level is the person's
/// own root, one folder (`platform::home`) that holds its manifest, lock, own items and store.
/// The user level has no targets: a user sync writes only its store and lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Project,
    User,
}

impl RootKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }

    /// The command `mars <args>` for a root of this kind: see [`Layout::cmd`].
    pub(crate) fn cmd(self, args: &str) -> String {
        match self {
            Self::User => crate::types::managed_cmd(&format!("mars --user {args}")).into_owned(),
            Self::Project => project_cmd(args),
        }
    }
}

/// The root this run acts on: one per CLI run, set once its root is known.
static RUN_ROOT: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();

/// Called once by the CLI, when the run's root is known.
pub fn set_run_root(layout: &Layout) {
    let _ = RUN_ROOT.set(layout.clone());
}

/// The root this run acts on, once known.
pub(crate) fn run_root() -> Option<&'static Layout> {
    RUN_ROOT.get()
}

/// [`Layout::cmd`] for the root this run acts on: for a message formatter that has no layout to
/// ask. A call site that has the layout its message is about asks it, since a run can read a
/// second root (a project sync reads the user level's lock).
pub(crate) fn root_cmd(args: &str) -> String {
    match run_root() {
        Some(root) => root.cmd(args),
        None => project_cmd(args),
    }
}

/// [`Layout::store_label`] for the root this run acts on, as [`root_cmd`] is.
pub(crate) fn store_label() -> String {
    match run_root() {
        Some(root) => root.store_label(),
        None => format!("{CANONICAL_TARGET_ROOT}/"),
    }
}

fn project_cmd(args: &str) -> String {
    crate::types::managed_cmd(&format!("mars {args}")).into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    kind: RootKind,
    root: PathBuf,
    store: PathBuf,
}

impl Layout {
    /// A project: everything lives under the directory holding `mars.toml`.
    pub fn project(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let store = root.join(CANONICAL_TARGET_ROOT);
        Self {
            kind: RootKind::Project,
            root,
            store,
        }
    }

    /// The user level: the store is the folder itself, beside the manifest. No store file is
    /// named like a root file (`mars.toml`, `mars.lock`, `mars.local.toml`, `.mars-src`, the
    /// curation files, `config.toml`, `curated.toml`), so the two share the folder.
    pub fn user(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            kind: RootKind::User,
            root: dir.clone(),
            store: dir,
        }
    }

    /// The user level at the folder `platform::home` resolves, canonical when it exists, or
    /// `None` when it cannot be located. Nothing is created.
    pub fn user_level() -> Option<Self> {
        crate::platform::home::mars_home()
            .map(|dir| Self::user(dunce::canonicalize(&dir).unwrap_or(dir)))
    }

    pub fn kind(&self) -> RootKind {
        self.kind
    }

    /// The command `mars <args>` for this root, as a message names it: `mars --user <args>` at
    /// the user level, through `types::managed_cmd`. A command for a fixed root (`mars --root ~
    /// unlink`) is written out with `managed_cmd`.
    pub(crate) fn cmd(&self, args: &str) -> String {
        self.kind.cmd(args)
    }

    /// This root's store, as a message names it: `.mars/` in a project, its path at the user
    /// level, whose store is its folder.
    pub(crate) fn store_label(&self) -> String {
        match self.kind {
            RootKind::User => format!("{}/", self.store.display()),
            RootKind::Project => format!("{CANONICAL_TARGET_ROOT}/"),
        }
    }

    /// The `root` object every `--json` output carries: which root the command used. Hosts
    /// parse it; keep one shape.
    pub fn root_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "path": self.root.to_string_lossy(),
        })
    }

    /// The directory the manifest sits in, against which its relative paths (path dependencies,
    /// overrides) resolve.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `mars.toml`.
    pub fn manifest(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// `mars.local.toml`.
    pub fn local_config(&self) -> PathBuf {
        self.root.join(LOCAL_CONFIG_FILE)
    }

    /// `mars.curated.toml`, the project's model curation tier.
    pub fn curated(&self) -> PathBuf {
        self.root.join("mars.curated.toml")
    }

    /// `mars.curated.local.toml`, this checkout's model curation tier.
    pub fn curated_local(&self) -> PathBuf {
        self.root.join("mars.curated.local.toml")
    }

    /// `mars.lock`.
    pub fn lock(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// The records only this machine has: provided content (`mars provide`) and the targets
    /// `mars.toml` does not name (`lock::machine`). The user level has neither, so it has no
    /// such file, and never reads one an old home project left in its folder.
    pub fn machine_lock(&self) -> Option<PathBuf> {
        (self.kind == RootKind::Project).then(|| self.store.join("machine.lock"))
    }

    /// `mars provide`'s record of the skills it copied into the store. A host provides to
    /// projects, so the user level has none, and never reads one an old home project left in its
    /// folder.
    pub fn provided_lock(&self) -> Option<PathBuf> {
        (self.kind == RootKind::Project).then(|| self.store.join("provided.lock"))
    }

    /// The canonical store, the `.mars` target.
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// The root's own items, authored by hand (`.mars-src`).
    pub fn local_source(&self) -> PathBuf {
        self.root.join(mars_schema::prepare::LOCAL_SOURCE_DIR)
    }

    /// The folder of a target id: `.mars` (the store), `.claude`, `.agents`, or a plain copy
    /// target.
    pub fn target(&self, id: &str) -> PathBuf {
        if id == CANONICAL_TARGET_ROOT {
            self.store.clone()
        } else {
            self.root.join(id)
        }
    }

    /// `relative` under the folder of target `id`.
    pub fn in_target(&self, id: &str, relative: impl AsRef<Path>) -> PathBuf {
        self.target(id).join(relative)
    }

    /// A file the target table names by its root-relative path (`.mcp.json`,
    /// `.codex/config.toml`, `.agents/plugins.json`), which need not sit in its target's folder.
    pub fn file(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}
