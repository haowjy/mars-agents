use std::collections::BTreeMap;
use std::path::Path;

use crate::config::PluginHarness;
use crate::error::MarsError;

use super::PluginBuildDestination;
use super::metadata::PackageMetadata;

pub(super) struct RootFilePlan {
    pub(super) root_files: BTreeMap<String, String>,
}

pub(super) fn plan(
    _package_root: &Path,
    _out_dir: &Path,
    _destination: &PluginBuildDestination,
    _metadata: &PackageMetadata,
    _harnesses: &[PluginHarness],
) -> Result<RootFilePlan, MarsError> {
    Ok(RootFilePlan {
        root_files: BTreeMap::new(),
    })
}

pub(super) fn apply(_plan: &RootFilePlan) -> Result<(), MarsError> {
    Ok(())
}
