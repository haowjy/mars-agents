use std::path::Path;

use indexmap::IndexMap;

use crate::config::{AgentOverlay, ModelPolicyRule};
use crate::error::{ConfigError, MarsError};
use crate::models::{self, ModelAlias};

pub(super) struct PolicyResolutionConfig {
    pub(super) aliases: IndexMap<String, ModelAlias>,
    pub(super) models_cache_ttl_hours: u32,
    pub(super) default_harness: Option<String>,
    pub(super) default_model: Option<String>,
    pub(super) harness_order: Option<Vec<String>>,
    pub(super) provider_order: Option<Vec<String>>,
    pub(super) linked_harnesses: Vec<String>,
    pub(super) agents: IndexMap<String, AgentOverlay>,
    pub(super) settings_model_policies: Vec<ModelPolicyRule>,
}

pub(super) fn load_policy_resolution_config(
    project_root: &Path,
) -> Result<PolicyResolutionConfig, MarsError> {
    let mut merged = models::builtin_aliases();
    let mut models_cache_ttl_hours = crate::config::Settings::default().models_cache_ttl_hours;
    let mut default_harness = None;
    let mut default_model = None;
    let mut harness_order = None;
    let mut provider_order = None;
    let mut linked_harnesses = Vec::new();
    let mut agents = IndexMap::new();
    let mut settings_model_policies = Vec::new();

    let merged_path = project_root.join(".mars").join("models-merged.json");
    if let Ok(content) = std::fs::read_to_string(&merged_path)
        && let Ok(cached) = serde_json::from_str::<IndexMap<String, ModelAlias>>(&content)
    {
        for (name, alias) in cached {
            merged.insert(name, alias);
        }
    }

    match crate::config::load_effective_project_config(project_root) {
        Ok(effective) => {
            models_cache_ttl_hours = effective.settings.models_cache_ttl_hours;
            default_harness = effective.settings.default_harness.clone();
            default_model = effective.settings.default_model.clone();
            harness_order = effective.settings.harness_order.clone();
            provider_order = effective.settings.provider_order.clone();
            linked_harnesses = effective.settings.linked_harnesses();
            agents = effective.agents.clone();
            settings_model_policies = effective.settings.model_policies.clone();

            for (name, alias) in &effective.models {
                merged.insert(name.clone(), alias.clone());
            }
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {}
        Err(err) => return Err(err),
    }

    let harness_order =
        harness_order.or_else(|| Some(crate::harness::registry::default_harness_order_names()));

    Ok(PolicyResolutionConfig {
        aliases: merged,
        models_cache_ttl_hours,
        default_harness,
        default_model,
        harness_order,
        provider_order,
        linked_harnesses,
        agents,
        settings_model_policies,
    })
}
