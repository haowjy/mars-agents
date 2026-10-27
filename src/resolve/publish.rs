//! Consumer-view dependency resolution for a package being published.

use std::path::{Path, PathBuf};

use crate::config::{Config, EffectiveConfig};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::source::GlobalCache;

use super::provider::RealSourceProvider;
use super::{ResolveOptions, ResolvedGraph, resolve};

/// The consumer-facing configuration view of a package being published.
///
/// Both the manifest and its effective form come from `mars.toml` only. Raw
/// source directories have no manifest but still use the same check and
/// lossiness paths through the default effective configuration.
#[derive(Debug, Clone)]
pub struct PublishLens {
    package_root: PathBuf,
    pub manifest: Option<Config>,
    pub effective: EffectiveConfig,
}

impl PublishLens {
    /// Prepare `package_root` (a package or a consumer project) and read its lens.
    pub fn load(
        package_root: &Path,
        diag: &mut DiagnosticCollector,
    ) -> Result<(Self, mars_schema::prepare::Prepared), MarsError> {
        let (prepared, manifest) = crate::package_prepare::prepare_project(package_root, diag)?;
        Ok((
            Self::from_prepared_manifest(package_root, manifest.as_deref())?,
            prepared,
        ))
    }

    pub(crate) fn from_prepared_manifest(
        package_root: &Path,
        normalized_manifest: Option<&[u8]>,
    ) -> Result<Self, MarsError> {
        let mut manifest = normalized_manifest
            .map(|bytes| {
                let content =
                    std::str::from_utf8(bytes).map_err(|error| MarsError::InvalidRequest {
                        message: format!("prepared mars.toml is not UTF-8: {error}"),
                    })?;
                crate::config::parse_config(content, &package_root.join("mars.toml"))
            })
            .transpose()?;
        let mut config = manifest.clone().unwrap_or_default();
        config.local_dependencies.clear();
        if let Some(manifest) = &mut manifest {
            manifest.local_dependencies.clear();
        }
        let (effective, diagnostics) = crate::config::merge_with_root(
            config,
            &crate::config::LocalLayers::default(),
            package_root,
        )?;
        if let Some(error) = diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.level == crate::diagnostic::DiagnosticLevel::Error)
        {
            return Err(MarsError::InvalidRequest {
                message: error.message,
            });
        }
        Ok(Self {
            package_root: package_root.to_path_buf(),
            manifest,
            effective,
        })
    }

    pub fn package_root(&self) -> &Path {
        &self.package_root
    }
}

/// Resolve the dependency graph consumers of this publish lens will see.
///
/// This lens reads only `mars.toml`: local dependencies and `mars.local.toml`
/// overrides are development inputs, so they cannot affect a published package.
/// It also resolves without a lock or staging root, leaving the package's
/// `mars.lock` and `.mars/` tree untouched. Source fetches use the global cache.
pub fn resolve_publish_graph(
    lens: &PublishLens,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedGraph, MarsError> {
    resolve_publish_graph_from_config(lens, &lens.effective, &ResolveOptions::default(), diag)
}

/// Resolve a caller-provided consumer view through the same publish lens.
///
/// Plugin builds use this to resolve an implicit consumer configuration before
/// staging the resulting graph. Keeping provider construction here preserves
/// the publish lens's no-lock, no-local-config boundary without teaching the
/// build module how sources are fetched.
pub(crate) fn resolve_publish_graph_from_config(
    lens: &PublishLens,
    effective: &EffectiveConfig,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedGraph, MarsError> {
    let cache = GlobalCache::new()?;
    let provider = RealSourceProvider::new(&cache, lens.package_root());

    resolve(effective, &provider, None, options, diag)
}
