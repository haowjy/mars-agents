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
/// own root, whose manifest sits in the Mars config folder and whose store is the Mars data
/// folder. The user level has no targets: a user sync writes only its store and lock.
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

    /// The user level: manifest, lock and own items in `config_dir`, the store in `data_dir`.
    pub fn user(config_dir: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            kind: RootKind::User,
            root: config_dir.into(),
            store: data_dir.into(),
        }
    }

    /// The user level at the folders `platform::config` resolves, canonical when they exist.
    /// Nothing is created. `Err` names the variable to set when a folder cannot be located
    /// (`platform::config::UNLOCATED_REASON`).
    pub fn user_level() -> Result<Self, &'static str> {
        let folder = |dir: Option<PathBuf>, variable: &'static str| {
            dir.map(|dir| dunce::canonicalize(&dir).unwrap_or(dir))
                .ok_or(variable)
        };
        Ok(Self::user(
            folder(
                crate::platform::config::mars_config_dir(),
                "MARS_CONFIG_DIR",
            )?,
            folder(crate::platform::config::mars_data_dir(), "MARS_DATA_DIR")?,
        ))
    }

    pub fn kind(&self) -> RootKind {
        self.kind
    }

    /// The `root` object every `--json` output carries: which root the command used and, for
    /// the user level, its config and store folders. Hosts parse it; keep one shape.
    pub fn root_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "kind": self.kind.as_str(),
            "path": self.root.to_string_lossy(),
        });
        if self.kind == RootKind::User {
            value["config_dir"] = self.root.to_string_lossy().into();
            value["store_dir"] = self.store.to_string_lossy().into();
        }
        value
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
