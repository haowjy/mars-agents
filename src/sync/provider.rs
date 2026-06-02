//! Real source provider implementation for the resolver.
//!
//! Bridges the resolver's trait-based interface to the concrete source module.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use crate::config::Manifest;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::resolve::{ManifestReader, SourceFetcher, VersionLister};
use crate::source::{self, AvailableVersion, GlobalCache, ResolvedRef};
use crate::types::CommitHash;
use crate::types::SourceUrl;

/// Real source provider that delegates to the source module.
///
/// Implements the SourceProvider trait so the resolver can fetch sources
/// and read manifests through a uniform interface.
pub(crate) struct RealSourceProvider<'a> {
    pub cache: &'a GlobalCache,
    pub project_root: &'a Path,
    version_cache: RefCell<HashMap<SourceUrl, Vec<AvailableVersion>>>,
}

impl<'a> RealSourceProvider<'a> {
    pub(crate) fn new(cache: &'a GlobalCache, project_root: &'a Path) -> Self {
        Self {
            cache,
            project_root,
            version_cache: RefCell::new(HashMap::new()),
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

impl SourceFetcher for RealSourceProvider<'_> {
    fn fetch_git_version(
        &self,
        url: &crate::types::SourceUrl,
        version: &AvailableVersion,
        source_name: &str,
        preferred_commit: Option<&str>,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        let fetch_options = source::git::FetchOptions {
            preferred_commit: preferred_commit.map(CommitHash::from),
        };
        source::git::fetch(
            url.as_ref(),
            Some(&version.tag),
            source_name,
            self.cache,
            &fetch_options,
            diag,
        )
    }

    fn fetch_git_ref(
        &self,
        url: &crate::types::SourceUrl,
        ref_name: &str,
        source_name: &str,
        preferred_commit: Option<&str>,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        let fetch_options = source::git::FetchOptions {
            preferred_commit: preferred_commit.map(CommitHash::from),
        };
        source::git::fetch(
            url.as_ref(),
            Some(ref_name),
            source_name,
            self.cache,
            &fetch_options,
            diag,
        )
    }

    fn fetch_git_commit(
        &self,
        url: &crate::types::SourceUrl,
        commit: &str,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError> {
        source::git::fetch_commit(url.as_ref(), commit, source_name, self.cache, diag)
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

impl ManifestReader for RealSourceProvider<'_> {
    fn read_manifest(
        &self,
        source_tree: &Path,
        diag: &mut DiagnosticCollector,
    ) -> Result<Option<Manifest>, MarsError> {
        let (manifest, diagnostics) = crate::config::load_manifest(source_tree)?;
        diag.extend(diagnostics);
        Ok(manifest)
    }
}
