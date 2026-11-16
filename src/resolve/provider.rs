//! What the resolver's answerer asks of git and the disk, and the real implementation.

use std::path::{Path, PathBuf};

use mars_schema::resolve::{Tag, sort_tags};

use crate::error::MarsError;
use crate::source::{self, AvailableVersion, GlobalCache};
use crate::types::SourceUrl;

pub trait SourceProvider {
    /// Every tag of the repository, with the commit it points at (peeled for an annotated tag).
    fn list_tags(&self, url: &SourceUrl) -> Result<Vec<Tag>, MarsError>;

    /// The commit a branch, `HEAD` or commit-as-ref names.
    fn resolve_ref(&self, url: &SourceUrl, reference: &str) -> Result<String, MarsError>;

    /// The content directory of `commit`. `tag` is the tag it was listed under, if it was.
    /// `LockedCommitUnreachable` is a commit the repository does not have; any other error says
    /// nothing about the commit.
    fn fetch_content(
        &self,
        url: &SourceUrl,
        commit: &str,
        tag: Option<&str>,
    ) -> Result<PathBuf, MarsError>;

    /// The canonical directory a path dependency names.
    fn fetch_path(&self, path: &Path, source_name: &str) -> Result<PathBuf, MarsError>;

    /// The tags that are versions, oldest first.
    fn list_versions(&self, url: &SourceUrl) -> Result<Vec<AvailableVersion>, MarsError> {
        Ok(sort_tags(self.list_tags(url)?)
            .into_iter()
            .map(|listed| AvailableVersion {
                tag: listed.tag,
                version: listed.version,
                commit_id: listed.commit,
            })
            .collect())
    }
}

pub(crate) struct RealSourceProvider<'a> {
    pub cache: &'a GlobalCache,
    pub project_root: &'a Path,
}

impl<'a> RealSourceProvider<'a> {
    pub(crate) fn new(cache: &'a GlobalCache, project_root: &'a Path) -> Self {
        Self {
            cache,
            project_root,
        }
    }
}

impl SourceProvider for RealSourceProvider<'_> {
    fn list_tags(&self, url: &SourceUrl) -> Result<Vec<Tag>, MarsError> {
        source::git::ls_remote_tags(url.as_ref())
    }

    fn resolve_ref(&self, url: &SourceUrl, reference: &str) -> Result<String, MarsError> {
        source::git::ls_remote_ref(url.as_ref(), reference)
    }

    fn fetch_content(
        &self,
        url: &SourceUrl,
        commit: &str,
        tag: Option<&str>,
    ) -> Result<PathBuf, MarsError> {
        source::git::fetch_content(url.as_ref(), commit, tag, self.cache)
    }

    fn fetch_path(&self, path: &Path, source_name: &str) -> Result<PathBuf, MarsError> {
        source::path::fetch_path(path, self.project_root, source_name)
    }
}
