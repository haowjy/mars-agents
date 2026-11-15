pub(crate) mod archive;
pub mod git;
pub(crate) mod git_cli;
pub mod parse;
pub mod path;
pub(crate) mod tree;

use std::path::PathBuf;
use std::sync::Arc;

use mars_schema::tree::FileTree;

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

    /// One directory per `(url, commit)`: a GitHub tarball extracted, or any other git source
    /// exported from its clone. Content directories are written whole and never changed.
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
    /// Where the files are: for a git source, its content directory in the cache, which holds
    /// exactly this commit (see `git_cli::fetch_git_content`).
    pub tree_path: PathBuf,
    /// The files at `tree_path`, read once by the provider. `None` for a path source, which is
    /// live: Mars never snapshots a directory the user is editing, and it may hold anything.
    pub tree: Option<Arc<FileTree>>,
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
