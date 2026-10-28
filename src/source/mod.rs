pub(crate) mod archive;
pub mod git;
pub(crate) mod git_cli;
pub mod parse;
pub mod path;

use std::path::PathBuf;

use crate::error::MarsError;
use crate::platform::cache::global_cache_root;
use crate::types::{CommitHash, SourceName, SourceUrl};

/// Global source cache under the OS cache root (or `MARS_CACHE_DIR`).
#[derive(Debug, Clone)]
pub struct GlobalCache {
    pub root: PathBuf,
}

impl GlobalCache {
    /// Locate the cache. Nothing is created here: the first fetch creates `git/` and `archives/`,
    /// so a run that fetches nothing works with a read-only cache root.
    ///
    /// Resolution order:
    /// 1. `MARS_CACHE_DIR`
    /// 2. OS cache directory + `mars/cache`
    /// 3. `{current_working_dir}/.mars/cache` fallback
    pub fn new() -> Result<Self, MarsError> {
        let root = global_cache_root()?;

        Ok(Self { root })
    }

    pub fn archives_dir(&self) -> PathBuf {
        self.root.join("archives")
    }

    pub fn git_dir(&self) -> PathBuf {
        self.root.join("git")
    }
}

/// A resolved source reference — pinned to a specific version/commit.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    pub source_name: SourceName,
    pub version: Option<semver::Version>,
    /// Original tag name (e.g., "v0.5.2")
    pub version_tag: Option<String>,
    pub commit: Option<CommitHash>,
    pub tree_path: PathBuf,
}

/// Available version from a git remote.
#[derive(Debug, Clone)]
pub struct AvailableVersion {
    pub tag: String,
    pub version: semver::Version,
    pub commit_id: String,
}

/// List available versions from a git remote (for resolution).
pub fn list_versions(
    url: &SourceUrl,
    cache: &GlobalCache,
) -> Result<Vec<AvailableVersion>, MarsError> {
    git::list_versions(url.as_ref(), cache)
}
