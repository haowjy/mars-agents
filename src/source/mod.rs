pub(crate) mod archive;
pub mod git;
pub(crate) mod git_cli;
pub mod parse;
pub mod path;

use std::path::PathBuf;

use crate::error::MarsError;
use crate::platform::cache::global_cache_root;
use crate::types::{CommitHash, SourceName, SourceUrl};

#[derive(Debug, Clone)]
pub struct GlobalCache {
    pub root: PathBuf,
}

impl GlobalCache {
    /// Locate the cache. Nothing is created here: the first fetch creates `git/` and `archives/`,
    /// so a run that fetches nothing works with a read-only cache root.
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

#[derive(Debug, Clone)]
pub struct ResolvedRef {
    pub source_name: SourceName,
    pub version: Option<semver::Version>,
    pub version_tag: Option<String>,
    pub commit: Option<CommitHash>,
    pub tree_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AvailableVersion {
    pub tag: String,
    pub version: semver::Version,
    pub commit_id: String,
}

pub fn list_versions(
    url: &SourceUrl,
    cache: &GlobalCache,
) -> Result<Vec<AvailableVersion>, MarsError> {
    git::list_versions(url.as_ref(), cache)
}
