use std::path::Path;

use indexmap::IndexMap;
use serde::Deserialize;

use crate::error::{ConfigError, MarsError};

use super::{
    AgentOverlay, LocalConfig, LocalModelVisibility, LocalSettings, ModelPolicyRule, Settings,
};

pub struct SettingsLayerInputs<'a> {
    pub user: Option<&'a LocalSettings>,
    pub project: Option<&'a Settings>,
    pub project_overlay: Option<&'a LocalSettings>,
    pub project_local: Option<&'a LocalSettings>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
struct ProjectSettingsOverlay {
    #[serde(default)]
    managed_root: Option<String>,
    #[serde(default)]
    targets: Option<Vec<String>>,
    #[serde(default)]
    model_visibility: Option<ProjectModelVisibilityOverlay>,
    #[serde(default)]
    models_cache_ttl_hours: Option<u32>,
    #[serde(default)]
    min_mars_version: Option<String>,
    #[serde(default)]
    default_harness: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    harness_order: Option<Vec<String>>,
    #[serde(default)]
    provider_order: Option<Vec<String>>,
    #[serde(default)]
    agent_emission: Option<super::AgentEmission>,
    #[serde(default, rename = "model-policies")]
    model_policies: Option<Vec<ModelPolicyRule>>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
struct ProjectModelVisibilityOverlay {
    #[serde(default)]
    include: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
}

impl From<ProjectModelVisibilityOverlay> for LocalModelVisibility {
    fn from(value: ProjectModelVisibilityOverlay) -> Self {
        Self {
            include: value.include,
            exclude: value.exclude,
        }
    }
}

impl From<ProjectSettingsOverlay> for LocalSettings {
    fn from(value: ProjectSettingsOverlay) -> Self {
        Self {
            managed_root: value.managed_root,
            targets: value.targets,
            model_visibility: value.model_visibility.map(Into::into),
            models_cache_ttl_hours: value.models_cache_ttl_hours,
            min_mars_version: value.min_mars_version,
            default_harness: value.default_harness,
            default_model: value.default_model,
            harness_order: value.harness_order,
            provider_order: value.provider_order,
            agent_emission: value.agent_emission,
            model_policies: value.model_policies,
        }
    }
}

pub fn load_project_settings_overlay(root: &Path) -> Result<Option<LocalSettings>, MarsError> {
    let path = root.join("mars.toml");
    let content = std::fs::read_to_string(&path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound { path: path.clone() }
        } else {
            ConfigError::Io(err)
        }
    })?;

    let parsed: toml::Value = toml::from_str(&content).map_err(ConfigError::Parse)?;
    let Some(settings_value) = parsed.get("settings") else {
        return Ok(None);
    };

    let settings: ProjectSettingsOverlay = settings_value
        .clone()
        .try_into()
        .map_err(ConfigError::Parse)?;
    Ok(Some(settings.into()))
}

pub fn layered_settings(inputs: SettingsLayerInputs<'_>) -> Settings {
    let mut merged = Settings::default();
    if let Some(user) = inputs.user {
        merged = user.overlay_settings(&merged);
    }

    if let Some(project_overlay) = inputs.project_overlay {
        merged = project_overlay.overlay_settings(&merged);
    } else if let Some(project) = inputs.project {
        merged = project.clone();
    }

    if let Some(project_local) = inputs.project_local {
        merged = project_local.overlay_settings(&merged);
    }
    merged
}

fn overlay_map_replace_by_key<V: Clone>(
    base: &IndexMap<String, V>,
    overlay: &IndexMap<String, V>,
) -> IndexMap<String, V> {
    let mut merged = base.clone();
    for (key, value) in overlay {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

pub fn overlay_models_replace_by_key(
    base: &IndexMap<String, crate::models::ModelAlias>,
    local: &LocalConfig,
) -> IndexMap<String, crate::models::ModelAlias> {
    overlay_map_replace_by_key(base, &local.models)
}

pub fn overlay_agent_overlays_replace_by_key(
    base: &IndexMap<String, AgentOverlay>,
    local: &LocalConfig,
) -> IndexMap<String, AgentOverlay> {
    overlay_map_replace_by_key(base, &local.agents)
}

pub fn merged_settings_model_policies(
    settings: &Settings,
    local: &LocalConfig,
) -> Vec<ModelPolicyRule> {
    merged_settings(settings, local).model_policies
}

pub fn merged_settings(settings: &Settings, local: &LocalConfig) -> Settings {
    layered_settings(SettingsLayerInputs {
        user: None,
        project: Some(settings),
        project_overlay: None,
        project_local: Some(&local.settings),
    })
}

impl LocalSettings {
    pub(crate) fn is_empty(&self) -> bool {
        self.managed_root.is_none()
            && self.targets.is_none()
            && self.model_visibility.is_none()
            && self.models_cache_ttl_hours.is_none()
            && self.min_mars_version.is_none()
            && self.default_harness.is_none()
            && self.default_model.is_none()
            && self.harness_order.is_none()
            && self.provider_order.is_none()
            && self.agent_emission.is_none()
            && self.model_policies.is_none()
    }

    pub(crate) fn overlay_settings(&self, base: &Settings) -> Settings {
        let mut merged = base.clone();

        if let Some(value) = &self.managed_root {
            merged.managed_root = Some(value.clone());
        }
        if let Some(value) = &self.targets {
            merged.targets = Some(value.clone());
        }
        if let Some(value) = &self.model_visibility {
            apply_model_visibility_overlay(&mut merged, value);
        }
        if let Some(value) = self.models_cache_ttl_hours {
            merged.models_cache_ttl_hours = value;
        }
        if let Some(value) = &self.min_mars_version {
            merged.min_mars_version = Some(value.clone());
        }
        if let Some(value) = &self.default_harness {
            merged.default_harness = Some(value.clone());
        }
        if let Some(value) = &self.default_model {
            merged.default_model = Some(value.clone());
        }
        if let Some(value) = &self.harness_order {
            merged.harness_order = Some(value.clone());
        }
        if let Some(value) = &self.provider_order {
            merged.provider_order = Some(value.clone());
        }
        if let Some(value) = &self.agent_emission {
            merged.agent_emission = Some(value.clone());
        }
        if let Some(value) = &self.model_policies {
            merged.model_policies = value.clone();
        }

        merged
    }
}

fn apply_model_visibility_overlay(merged: &mut Settings, overlay: &LocalModelVisibility) {
    if let Some(include) = &overlay.include {
        merged.model_visibility.include = Some(include.clone());
    }
    if let Some(exclude) = &overlay.exclude {
        merged.model_visibility.exclude = Some(exclude.clone());
    }
}
