//! Consumer-view dependency resolution for a package being published.

use std::path::{Path, PathBuf};

use crate::config::{Config, EffectiveConfig, LocalConfig};
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::source::GlobalCache;

use super::provider::RealSourceProvider;
use super::{ResolveOptions, ResolvedGraph, resolve};

/// The consumer-facing configuration view of a package being published.
///
/// Both the authored config and its effective form come from `mars.toml` only.
/// A missing manifest is represented by defaults so raw source directories can
/// still use the same check and lossiness paths.
#[derive(Debug, Clone)]
pub struct PublishLens {
    package_root: PathBuf,
    manifest_present: bool,
    pub config: Config,
    pub effective: EffectiveConfig,
}

impl PublishLens {
    pub fn load(package_root: &Path, diag: &mut DiagnosticCollector) -> Result<Self, MarsError> {
        let (mut config, manifest_present) = match crate::config::load(package_root) {
            Ok(config) => (config, true),
            Err(MarsError::Config(ConfigError::NotFound { .. })) => (Config::default(), false),
            Err(err) => return Err(err),
        };
        config.local_dependencies.clear();
        let (effective, diagnostics) =
            crate::config::merge_with_root(config.clone(), LocalConfig::default(), package_root)?;
        diag.extend(diagnostics);

        Ok(Self {
            package_root: package_root.to_path_buf(),
            manifest_present,
            config,
            effective,
        })
    }

    pub fn package_root(&self) -> &Path {
        &self.package_root
    }

    pub fn manifest_config(&self) -> Option<&Config> {
        self.manifest_present.then_some(&self.config)
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
    let cache = GlobalCache::new()?;
    let provider = RealSourceProvider::new(&cache, lens.package_root());

    resolve(
        &lens.effective,
        &provider,
        None,
        &ResolveOptions::default(),
        diag,
    )
}
