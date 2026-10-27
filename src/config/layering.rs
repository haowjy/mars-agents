use indexmap::IndexMap;

use super::targets::{LinkSource, TargetOrigin, TargetSource};

use super::{
    AgentOverlay, AgentOverlayTools, Config, LocalConfig, LocalSettings, Settings, SkillOverlay,
};
use mars_schema::permission_set::{PermissionSet, compose_permission_sets};

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
    higher: &IndexMap<String, crate::models::ModelAlias>,
) -> IndexMap<String, crate::models::ModelAlias> {
    overlay_map_replace_by_key(base, higher)
}

pub fn overlay_agent_overlays(
    base: &IndexMap<String, AgentOverlay>,
    higher_overlays: &IndexMap<String, AgentOverlay>,
) -> IndexMap<String, AgentOverlay> {
    let mut merged = base.clone();
    for (name, higher) in higher_overlays {
        let overlay = match base.get(name) {
            Some(lower) => compose_agent_overlay(lower, higher),
            None => higher.clone(),
        };
        merged.insert(name.clone(), overlay);
    }
    merged
}

pub fn overlay_skill_overlays(
    base: &IndexMap<String, SkillOverlay>,
    local: &IndexMap<String, SkillOverlay>,
) -> IndexMap<String, SkillOverlay> {
    let mut merged = base.clone();
    for (name, higher) in local {
        let overlay = match base.get(name) {
            Some(lower) => compose_skill_overlay(lower, higher),
            None => higher.clone(),
        };
        merged.insert(name.clone(), overlay);
    }
    merged
}

fn compose_agent_overlay(lower: &AgentOverlay, higher: &AgentOverlay) -> AgentOverlay {
    let mut merged = higher.clone();
    merged.network_allow = compose_permission_sets(&lower.network_allow, &higher.network_allow);
    merged.network_deny = compose_permission_sets(&lower.network_deny, &higher.network_deny);
    merged.tools = compose_tool_permission_sets(&lower.tools, &higher.tools);
    merged
}

fn compose_skill_overlay(_lower: &SkillOverlay, higher: &SkillOverlay) -> SkillOverlay {
    higher.clone()
}

fn compose_tool_permission_sets(
    lower: &AgentOverlayTools,
    higher: &AgentOverlayTools,
) -> AgentOverlayTools {
    AgentOverlayTools {
        allowed: compose_optional_permission_sets(lower.allowed.as_ref(), higher.allowed.as_ref()),
        disallowed: compose_optional_permission_sets(
            lower.disallowed.as_ref(),
            higher.disallowed.as_ref(),
        ),
        unknown: higher.unknown.clone(),
    }
}

fn compose_optional_permission_sets<T: Clone + Eq>(
    lower: Option<&PermissionSet<T>>,
    higher: Option<&PermissionSet<T>>,
) -> Option<PermissionSet<T>> {
    match (lower, higher) {
        (Some(lower), Some(higher)) => Some(compose_permission_sets(lower, higher)),
        (Some(layer), None) | (None, Some(layer)) => Some(layer.clone()),
        (None, None) => None,
    }
}

/// Everything the three config files contribute, composed bottom to top:
/// user `config.toml` < `mars.toml` < `mars.local.toml`.
pub(super) struct ComposedLayers {
    pub settings: Settings,
    pub target_source: TargetSource,
    pub models: IndexMap<String, crate::models::ModelAlias>,
    pub agents: IndexMap<String, AgentOverlay>,
    pub skills: IndexMap<String, SkillOverlay>,
}

pub(super) fn compose_layers(
    config: &Config,
    local: &LocalConfig,
    user: &LocalConfig,
) -> ComposedLayers {
    let (settings, target_source) = [
        &user.settings,
        &LocalSettings::from(&config.settings),
        &local.settings,
    ]
    .into_iter()
    .fold(
        (Settings::default(), TargetSource::default()),
        |(settings, _), layer| layer.overlay_settings(&settings),
    );
    // The fold's target source is only right for its last step, which sees the project's targets as the base.
    ComposedLayers {
        settings,
        target_source,
        models: overlay_models_replace_by_key(
            &overlay_models_replace_by_key(&user.models, &config.models),
            &local.models,
        ),
        agents: overlay_agent_overlays(
            &overlay_agent_overlays(&user.agents, &config.agents),
            &local.agents,
        ),
        skills: overlay_skill_overlays(
            &overlay_skill_overlays(&user.skills, &config.skills),
            &local.skills,
        ),
    }
}

impl From<&Settings> for LocalSettings {
    /// A project's `[settings]` as a layer: every field it sets, no more.
    fn from(project: &Settings) -> Self {
        let Settings {
            managed_root,
            targets,
            models_cache_ttl_hours,
            min_mars_version,
            default_harness,
            default_model,
            default_network,
            network_allow,
            network_deny,
            harness_order,
            provider_order,
            catalog_providers,
            agent_emission,
            meridian,
            model_policies,
            unknown,
        } = project.clone();
        LocalSettings {
            managed_root,
            targets,
            models_cache_ttl_hours,
            min_mars_version,
            default_harness,
            default_model,
            default_network,
            network_allow: Some(network_allow),
            network_deny: Some(network_deny),
            harness_order,
            provider_order,
            catalog_providers,
            agent_emission,
            meridian,
            model_policies,
            unknown,
        }
    }
}

impl LocalSettings {
    pub(crate) fn is_empty(&self) -> bool {
        self.managed_root.is_none()
            && self.targets.is_none()
            && self.models_cache_ttl_hours.is_none()
            && self.min_mars_version.is_none()
            && self.default_harness.is_none()
            && self.default_model.is_none()
            && self.default_network.is_none()
            && self.network_allow.is_none()
            && self.network_deny.is_none()
            && self.harness_order.is_none()
            && self.provider_order.is_none()
            && self.catalog_providers.is_none()
            && self.agent_emission.is_none()
            && self.meridian.is_empty()
            && self.model_policies.is_none()
            && self.unknown.is_empty()
    }

    pub(crate) fn overlay_settings(&self, base: &Settings) -> (Settings, TargetSource) {
        let mut merged = base.clone();
        let field = base.effective_links().source;
        let mut target_source = TargetSource {
            field,
            origin: if field == LinkSource::None {
                TargetOrigin::Unset
            } else {
                TargetOrigin::Project
            },
            path: None,
        };

        if let Some(value) = &self.managed_root {
            merged.managed_root = Some(value.clone());
            if merged.targets.is_none() {
                target_source.field = LinkSource::ManagedRoot;
                target_source.origin = TargetOrigin::Local;
            }
        }
        if let Some(value) = &self.targets {
            merged.targets = Some(value.clone());
            target_source.field = LinkSource::Targets;
            target_source.origin = TargetOrigin::Local;
        }
        if let Some(value) = self.models_cache_ttl_hours {
            merged.models_cache_ttl_hours = Some(value);
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
        if let Some(value) = &self.default_network {
            merged.default_network = Some(value.clone());
        }
        if let Some(value) = &self.network_allow {
            merged.network_allow = compose_permission_sets(&merged.network_allow, value);
        }
        if let Some(value) = &self.network_deny {
            merged.network_deny = compose_permission_sets(&merged.network_deny, value);
        }
        if let Some(value) = &self.harness_order {
            merged.harness_order = Some(value.clone());
        }
        if let Some(value) = &self.provider_order {
            merged.provider_order = Some(value.clone());
        }
        if let Some(value) = &self.catalog_providers {
            merged.catalog_providers = Some(value.clone());
        }
        if let Some(value) = &self.agent_emission {
            merged.agent_emission = Some(value.clone());
        }
        if let Some(value) = &self.meridian.agent_copy {
            merged.meridian.agent_copy = Some(value.clone());
        }
        if let Some(value) = &self.meridian.fanout {
            merged.meridian.fanout = Some(value.clone());
        }
        if let Some(value) = &self.model_policies {
            merged.model_policies = Some(value.clone());
        }

        (merged, target_source)
    }
}
