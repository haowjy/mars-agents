use indexmap::IndexMap;

use super::targets::{LinkSource, TargetOrigin, TargetSource};

use super::{
    AgentOverlay, AgentOverlayTools, LocalConfig, LocalSettings, Settings, SkillOverlay,
    ToolOverlayMigration, ToolOverlaySource,
};
use crate::permission_set::{PermissionSet, compose_permission_sets};

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

pub fn overlay_agent_overlays(
    base: &IndexMap<String, AgentOverlay>,
    local: &LocalConfig,
) -> IndexMap<String, AgentOverlay> {
    let mut merged = base
        .iter()
        .map(|(name, overlay)| {
            let mut overlay = overlay.clone();
            let project_tools = overlay.tools.clone();
            add_tool_migration(&mut overlay.tools, Some(&project_tools), None);
            (name.clone(), overlay)
        })
        .collect::<IndexMap<_, _>>();
    for (name, higher) in &local.agents {
        let overlay = match base.get(name) {
            Some(lower) => compose_agent_overlay(lower, higher),
            None => {
                let mut overlay = higher.clone();
                let local_tools = overlay.tools.clone();
                add_tool_migration(&mut overlay.tools, None, Some(&local_tools));
                overlay
            }
        };
        merged.insert(name.clone(), overlay);
    }
    merged
}

pub fn overlay_skill_overlays(
    base: &IndexMap<String, SkillOverlay>,
    local: &IndexMap<String, SkillOverlay>,
) -> IndexMap<String, SkillOverlay> {
    let mut merged = base
        .iter()
        .map(|(name, overlay)| {
            let mut overlay = overlay.clone();
            let project_tools = overlay.tools.clone();
            add_tool_migration(&mut overlay.tools, Some(&project_tools), None);
            (name.clone(), overlay)
        })
        .collect::<IndexMap<_, _>>();
    for (name, higher) in local {
        let overlay = match base.get(name) {
            Some(lower) => compose_skill_overlay(lower, higher),
            None => {
                let mut overlay = higher.clone();
                let local_tools = overlay.tools.clone();
                add_tool_migration(&mut overlay.tools, None, Some(&local_tools));
                overlay
            }
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
    add_tool_migration(&mut merged.tools, Some(&lower.tools), Some(&higher.tools));
    merged
}

fn compose_skill_overlay(lower: &SkillOverlay, higher: &SkillOverlay) -> SkillOverlay {
    let mut merged = higher.clone();
    merged.tools = compose_tool_permission_sets(&lower.tools, &higher.tools);
    add_tool_migration(&mut merged.tools, Some(&lower.tools), Some(&higher.tools));
    merged
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
        migration: None,
    }
}

fn add_tool_migration(
    effective: &mut AgentOverlayTools,
    project: Option<&AgentOverlayTools>,
    local: Option<&AgentOverlayTools>,
) {
    let previous = local
        .or(project)
        .expect("an effective overlay has a source");
    effective.migration = Some(ToolOverlayMigration {
        previous_allowed: previous.allowed.clone(),
        previous_disallowed: previous.disallowed.clone(),
        allowed_source: plain_list_source(
            project.and_then(|tools| tools.allowed.as_ref()),
            local.and_then(|tools| tools.allowed.as_ref()),
        ),
        disallowed_source: plain_list_source(
            project.and_then(|tools| tools.disallowed.as_ref()),
            local.and_then(|tools| tools.disallowed.as_ref()),
        ),
    });
}

fn plain_list_source<T>(
    project: Option<&PermissionSet<T>>,
    local: Option<&PermissionSet<T>>,
) -> Option<ToolOverlaySource> {
    if local.is_some_and(is_nonempty_plain_list) {
        Some(ToolOverlaySource::Local)
    } else if project.is_some_and(is_nonempty_plain_list) {
        Some(ToolOverlaySource::Project)
    } else {
        None
    }
}

fn is_nonempty_plain_list<T>(set: &PermissionSet<T>) -> bool {
    matches!(set, PermissionSet::Add(entries) if !entries.is_empty())
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
