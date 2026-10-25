//! Consumer-view dependency resolution for a package being published.

use std::path::Path;

use crate::config::LocalConfig;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::source::GlobalCache;

use super::provider::RealSourceProvider;
use super::{ResolveOptions, ResolvedGraph, resolve};

/// Resolve the dependency graph consumers of `package_root` will see.
///
/// This lens reads only `mars.toml`: local dependencies and `mars.local.toml`
/// overrides are development inputs, so they cannot affect a published package.
/// It also resolves without a lock or staging root, leaving the package's
/// `mars.lock` and `.mars/` tree untouched. Source fetches use the global cache.
pub fn resolve_publish_graph(package_root: &Path) -> Result<ResolvedGraph, MarsError> {
    let mut config = crate::config::load(package_root)?;
    config.local_dependencies.clear();
    let (effective, _diagnostics) =
        crate::config::merge_with_root(config, LocalConfig::default(), package_root)?;

    let cache = GlobalCache::new()?;
    let provider = RealSourceProvider::new(&cache, package_root);
    let mut diag = DiagnosticCollector::new();

    resolve(
        &effective,
        &provider,
        None,
        &ResolveOptions::default(),
        &mut diag,
    )
}
