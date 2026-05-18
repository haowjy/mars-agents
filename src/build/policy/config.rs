use std::path::Path;

use indexmap::IndexMap;

use crate::error::{ConfigError, MarsError};
use crate::models::{self, ModelAlias};

pub(super) struct PolicyResolutionConfig {
    pub(super) aliases: IndexMap<String, ModelAlias>,
    pub(super) default_harness: Option<String>,
    pub(super) harness_order: Option<Vec<String>>,
}

pub(super) fn load_policy_resolution_config(
    project_root: &Path,
) -> Result<PolicyResolutionConfig, MarsError> {
    let mut merged = models::builtin_aliases();
    let mut default_harness = None;
    let mut harness_order = None;

    let merged_path = project_root.join(".mars").join("models-merged.json");
    if let Ok(content) = std::fs::read_to_string(&merged_path)
        && let Ok(cached) = serde_json::from_str::<IndexMap<String, ModelAlias>>(&content)
    {
        for (name, alias) in cached {
            merged.insert(name, alias);
        }
    }

    match crate::config::load(project_root) {
        Ok(config) => {
            default_harness = config.settings.default_harness.clone();
            harness_order = config.settings.harness_order.clone();
            for (name, alias) in &config.models {
                merged.insert(name.clone(), alias.clone());
            }
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {}
        Err(err) => return Err(err),
    }

    Ok(PolicyResolutionConfig {
        aliases: merged,
        default_harness,
        harness_order,
    })
}
