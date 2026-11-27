//! Where a Mars root keeps its files: the one place that turns a root into the paths of its
//! manifest, locks, store, local source and targets. Code that needs one of these paths asks the
//! layout; it never joins a file name onto the root itself.

use std::path::{Path, PathBuf};

use crate::lock::CANONICAL_TARGET_ROOT;

/// The file names diagnostics use for a project's config files.
pub(crate) const CONFIG_FILE: &str = "mars.toml";
pub(crate) const LOCAL_CONFIG_FILE: &str = "mars.local.toml";
pub(crate) const LOCK_FILE: &str = "mars.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    store: PathBuf,
}

impl Layout {
    /// A project: everything lives under the directory holding `mars.toml`.
    pub fn project(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let store = root.join(CANONICAL_TARGET_ROOT);
        Self { root, store }
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
    /// `mars.toml` does not name (`lock::machine`).
    pub fn machine_lock(&self) -> PathBuf {
        self.store.join("machine.lock")
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
