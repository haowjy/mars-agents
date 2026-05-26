use indexmap::IndexMap;

use super::{AgentOverlay, LocalConfig, LocalModelVisibility, LocalSettings, Settings};

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

pub fn merged_settings(settings: &Settings, local: &LocalConfig) -> Settings {
    local.settings.overlay_settings(settings)
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
