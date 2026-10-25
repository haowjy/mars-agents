use std::path::Path;

use crate::config::PluginHarness;
use crate::error::MarsError;

use super::PackageMetadata;

pub(super) struct RootFilePlan;

pub(super) fn plan(
    _package_root: &Path,
    _out_dir: &Path,
    _metadata: &PackageMetadata,
    _harnesses: &[PluginHarness],
) -> Result<RootFilePlan, MarsError> {
    Ok(RootFilePlan)
}

pub(super) fn apply(_plan: &RootFilePlan) -> Result<(), MarsError> {
    Ok(())
}
