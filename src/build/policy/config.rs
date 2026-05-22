use std::path::Path;

use indexmap::IndexMap;

use crate::config::{AgentOverlay, ModelPolicyRule};
use crate::error::{ConfigError, MarsError};
use crate::models::{self, ModelAlias};

pub(super) struct PolicyResolutionConfig {
    pub(super) aliases: IndexMap<String, ModelAlias>,
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

    match crate::config::load(project_root) {
        Ok(config) => {
            default_harness = config.settings.default_harness.clone();
            default_model = config.settings.default_model.clone();
            harness_order = config.settings.harness_order.clone();
            provider_order = config.settings.provider_order.clone();
            linked_harnesses = config.settings.linked_harnesses();
            agents = config.agents.clone();
            for (name, alias) in &config.models {
                merged.insert(name.clone(), alias.clone());
            }
            let local = crate::config::load_local(project_root)?;
            agents = crate::config::merged_agent_overlays(&agents, &local);
            settings_model_policies =
                crate::config::merged_settings_model_policies(&config.settings, &local);
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {}
        Err(err) => return Err(err),
    }

    Ok(PolicyResolutionConfig {
        aliases: merged,
        default_harness,
        default_model,
        harness_order,
        provider_order,
        linked_harnesses,
        agents,
        settings_model_policies,
    })
}
