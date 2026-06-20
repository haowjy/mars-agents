//! Shared helpers for `mars models` CLI subcommands.

use crate::error::MarsError;
use crate::models::{self, ModelAlias};

pub(super) fn load_project_config_layers_optional(
    project_root: &std::path::Path,
) -> Result<Option<crate::config::LoadedProjectConfig>, MarsError> {
    match crate::config::load_project_config_layers(project_root) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(MarsError::Config(crate::error::ConfigError::NotFound { .. })) => Ok(None),
        Err(err) => Err(err),
    }
}

pub(super) fn models_cache_ttl_hours(
    project_config: Option<&crate::config::LoadedProjectConfig>,
) -> u32 {
    project_config
        .map(|loaded| loaded.effective.settings.models_cache_ttl_hours)
        .unwrap_or_else(|| crate::config::Settings::default().models_cache_ttl_hours)
}

/// Load model aliases by combining lock-persisted dependency aliases with effective
/// project/local consumer aliases.
pub(super) fn load_merged_aliases(
    project_root: &std::path::Path,
    project_config: Option<&crate::config::LoadedProjectConfig>,
) -> Result<indexmap::IndexMap<String, ModelAlias>, MarsError> {
    let lock = crate::lock::load_for_runtime_aliases(project_root)?;
    Ok(models::merged_runtime_aliases(
        &lock.dependency_model_aliases,
        project_config.map(|loaded| &loaded.effective.models),
    ))
}
