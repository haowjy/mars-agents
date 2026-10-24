use indexmap::IndexMap;

use super::targets::{LinkSource, TargetOrigin, TargetSource};

use super::{AgentOverlay, LocalConfig, LocalSettings, Settings, SkillOverlay};
use crate::permission_set::{PermissionSet, compose_permission_sets};

fn overlay_map_by_key<V: Clone>(
    base: &IndexMap<String, V>,
    overlay: &IndexMap<String, V>,
    merge: impl Fn(&V, &V) -> V,
) -> IndexMap<String, V> {
    let mut merged = base.clone();
    for (key, value) in overlay {
        let value = merged
            .get(key)
            .map_or_else(|| value.clone(), |lower| merge(lower, value));
        merged.insert(key.clone(), value);
    }
    merged
}

pub fn overlay_models_replace_by_key(
    base: &IndexMap<String, crate::models::ModelAlias>,
    local: &LocalConfig,
) -> IndexMap<String, crate::models::ModelAlias> {
    overlay_map_by_key(base, &local.models, |_, higher| higher.clone())
}

pub fn overlay_agent_overlays_by_field(
    base: &IndexMap<String, AgentOverlay>,
    local: &LocalConfig,
) -> IndexMap<String, AgentOverlay> {
    overlay_map_by_key(base, &local.agents, compose_agent_overlay)
}

pub fn overlay_skill_overlays_by_field(
    base: &IndexMap<String, SkillOverlay>,
    local: &IndexMap<String, SkillOverlay>,
) -> IndexMap<String, SkillOverlay> {
    overlay_map_by_key(base, local, compose_skill_overlay)
}

fn compose_agent_overlay(lower: &AgentOverlay, higher: &AgentOverlay) -> AgentOverlay {
    let mut unknown = lower.unknown.clone();
    unknown.extend(higher.unknown.clone());
    AgentOverlay {
        description: higher
            .description
            .clone()
            .or_else(|| lower.description.clone()),
        model: higher.model.clone().or_else(|| lower.model.clone()),
        harness: higher.harness.clone().or_else(|| lower.harness.clone()),
        effort: higher.effort.clone().or_else(|| lower.effort.clone()),
        approval: higher.approval.clone().or_else(|| lower.approval.clone()),
        sandbox: higher.sandbox.clone().or_else(|| lower.sandbox.clone()),
        network: higher.network.clone().or_else(|| lower.network.clone()),
        network_allow: compose_permission_sets(&lower.network_allow, &higher.network_allow),
        network_deny: compose_permission_sets(&lower.network_deny, &higher.network_deny),
        autocompact: higher.autocompact.or(lower.autocompact),
        autocompact_pct: higher.autocompact_pct.or(lower.autocompact_pct),
        model_invocable: higher.model_invocable.or(lower.model_invocable),
        user_invocable: higher.user_invocable.or(lower.user_invocable),
        tools: super::AgentOverlayTools {
            allowed: compose_optional_permission_sets(
                lower.tools.allowed.as_ref(),
                higher.tools.allowed.as_ref(),
            ),
            disallowed: compose_optional_permission_sets(
                lower.tools.disallowed.as_ref(),
                higher.tools.disallowed.as_ref(),
            ),
        },
        model_policies: higher
            .model_policies
            .clone()
            .or_else(|| lower.model_policies.clone()),
        unknown,
    }
}

fn compose_skill_overlay(lower: &SkillOverlay, higher: &SkillOverlay) -> SkillOverlay {
    SkillOverlay {
        description: higher
            .description
            .clone()
            .or_else(|| lower.description.clone()),
        model_invocable: higher.model_invocable.or(lower.model_invocable),
        user_invocable: higher.user_invocable.or(lower.user_invocable),
        tools: super::AgentOverlayTools {
            allowed: compose_optional_permission_sets(
                lower.tools.allowed.as_ref(),
                higher.tools.allowed.as_ref(),
            ),
            disallowed: compose_optional_permission_sets(
                lower.tools.disallowed.as_ref(),
                higher.tools.disallowed.as_ref(),
            ),
        },
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

pub fn merged_settings(settings: &Settings, local: &LocalConfig) -> Settings {
    local.settings.overlay_settings(settings).0
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
            merged.model_policies = value.clone();
        }

        (merged, target_source)
    }
}
