//! Real source provider implementation for dependency resolution.
//!
//! Bridges the resolver's trait-based interface to the concrete source module
//! for sync and other production resolution lenses.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mars_schema::tree::FileTree;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::resolve::{SourceFetcher, VersionLister};
use crate::source::{self, AvailableVersion, GlobalCache, ResolvedRef};
use crate::types::SourceUrl;

/// Real source provider that delegates to the source module.
///
/// Implements the SourceProvider trait so the resolver can fetch sources
/// and read manifests through a uniform interface.
pub(crate) struct RealSourceProvider<'a> {
    pub cache: &'a GlobalCache,
    pub project_root: &'a Path,
    version_cache: RefCell<HashMap<SourceUrl, Vec<AvailableVersion>>>,
    trees: RefCell<HashMap<PathBuf, Arc<FileTree>>>,
}

impl<'a> RealSourceProvider<'a> {
    pub(crate) fn new(cache: &'a GlobalCache, project_root: &'a Path) -> Self {
        Self {
            cache,
            project_root,
            version_cache: RefCell::new(HashMap::new()),
            trees: RefCell::new(HashMap::new()),
        }
    }
}

impl VersionLister for RealSourceProvider<'_> {
    fn list_versions(
        &self,
        url: &crate::types::SourceUrl,
    ) -> Result<Vec<AvailableVersion>, MarsError> {
        if let Some(available) = self.version_cache.borrow().get(url) {
            return Ok(available.clone());
        }
        let available = source::list_versions(url, self.cache)?;
        self.version_cache
            .borrow_mut()
            .insert(url.clone(), available.clone());
        Ok(available)
    }
}

impl RealSourceProvider<'_> {
    /// Attach the files of a fetched git source. A restart fetches the same commit again, so the
    /// read is kept per content directory, which holds one commit for good.
    fn with_tree(&self, mut resolved: ResolvedRef) -> Result<ResolvedRef, MarsError> {
        let cached = self.trees.borrow().get(&resolved.tree_path).cloned();
        let tree = match cached {
            Some(tree) => tree,
            None => {
                let tree = Arc::new(source::tree::read_tree(&resolved.tree_path)?);
                self.trees
                    .borrow_mut()
                    .insert(resolved.tree_path.clone(), Arc::clone(&tree));
                tree
            }
        };
        resolved.tree = Some(tree);
        Ok(resolved)
    }
}

impl SourceFetcher for RealSourceProvider<'_> {
    fn fetch_git_version(
        &self,
        url: &crate::types::SourceUrl,
        version: &AvailableVersion,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        let resolved = source::git::fetch(
            url.as_ref(),
            Some(&version.tag),
            source_name,
            self.cache,
            diag,
        )?;
        self.with_tree(resolved)
    }

    fn fetch_git_ref(
        &self,
        url: &crate::types::SourceUrl,
        ref_name: &str,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        let resolved =
            source::git::fetch(url.as_ref(), Some(ref_name), source_name, self.cache, diag)?;
        self.with_tree(resolved)
    }

    fn fetch_git_commit(
        &self,
        url: &crate::types::SourceUrl,
        commit: &str,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        let resolved =
            source::git::fetch_commit(url.as_ref(), commit, source_name, self.cache, diag)?;
        self.with_tree(resolved)
    }

    fn fetch_path(
        &self,
        path: &Path,
        source_name: &str,
        _diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        source::path::fetch_path(path, self.project_root, source_name)
    }
}
