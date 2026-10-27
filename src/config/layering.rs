use indexmap::IndexMap;

use std::path::{Path, PathBuf};

use super::targets::{LinkSource, TargetOrigin, TargetSource};

use super::{
    AgentOverlay, AgentOverlayTools, LocalConfig, LocalSettings, MeridianSettings, Settings,
    SkillOverlay,
};
use mars_schema::permission_set::{PermissionSet, compose_permission_sets};

fn overlay_map_replace_by_key<K: Clone + Eq + std::hash::Hash, V: Clone>(
    base: &IndexMap<K, V>,
    overlay: &IndexMap<K, V>,
) -> IndexMap<K, V> {
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
    let mut merged = base.clone();
    for (name, higher) in &local.agents {
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

/// One local-config file with the path it was read from, so a message about a value can name
/// the file that holds it.
#[derive(Debug, Clone, Default)]
pub struct LocalLayer {
    pub path: PathBuf,
    pub config: LocalConfig,
}

/// The local configs that sit above `mars.toml`: the user `local.toml`, then the project
/// `mars.local.toml`. Kept apart until a reader needs the effective value, so provenance
/// survives and only the project layer can be saved.
#[derive(Debug, Clone, Default)]
pub struct LocalLayers {
    pub user: LocalLayer,
    pub project: LocalLayer,
}

impl LocalLayers {
    /// Only a project layer, for callers with no user file (publish lens, tests).
    pub fn project_only(config: LocalConfig, root: &Path) -> Self {
        Self {
            user: LocalLayer::default(),
            project: LocalLayer {
                path: root.join(super::LOCAL_CONFIG_FILE),
                config,
            },
        }
    }

    /// The layers in precedence order, lowest first.
    pub fn in_order(&self) -> [&LocalLayer; 2] {
        [&self.user, &self.project]
    }

    /// Both layers folded into one `LocalConfig`, so the merge over `mars.toml` yields
    /// `mars.toml < user local < project local`. Keys follow the per-section rules above;
    /// the project layer wins.
    pub fn merged(&self) -> LocalConfig {
        let (user, project) = (&self.user.config, &self.project.config);
        LocalConfig {
            overrides: overlay_map_replace_by_key(&user.overrides, &project.overrides),
            models: overlay_map_replace_by_key(&user.models, &project.models),
            agents: overlay_agent_overlays(&user.agents, project),
            skills: overlay_skill_overlays(&user.skills, &project.skills),
            settings: user.settings.layered_under(&project.settings),
        }
    }

    /// The layer whose `[settings]` supplied the winning target field of the merged config.
    pub fn target_layer(&self, field: LinkSource) -> &LocalLayer {
        let in_project = match field {
            LinkSource::Targets => self.project.config.settings.targets.is_some(),
            LinkSource::ManagedRoot => self.project.config.settings.managed_root.is_some(),
            LinkSource::None => false,
        };
        if in_project { &self.project } else { &self.user }
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

    /// `self` with every field `higher` sets replaced; network host sets compose. `higher` is
    /// destructured exhaustively, so a new field fails to compile until it is layered here.
    fn layered_under(&self, higher: &LocalSettings) -> LocalSettings {
        let LocalSettings {
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
            meridian: MeridianSettings { agent_copy, fanout },
            model_policies,
            unknown,
        } = higher;
        let mut merged = self.clone();
        macro_rules! take {
            ($($field:ident),*) => {$(
                if $field.is_some() {
                    merged.$field = $field.clone();
                }
            )*};
        }
        take!(
            managed_root,
            targets,
            models_cache_ttl_hours,
            min_mars_version,
            default_harness,
            default_model,
            default_network,
            harness_order,
            provider_order,
            catalog_providers,
            agent_emission,
            model_policies
        );
        merged.network_allow =
            compose_optional_permission_sets(self.network_allow.as_ref(), network_allow.as_ref());
        merged.network_deny =
            compose_optional_permission_sets(self.network_deny.as_ref(), network_deny.as_ref());
        if agent_copy.is_some() {
            merged.meridian.agent_copy = agent_copy.clone();
        }
        if fanout.is_some() {
            merged.meridian.fanout = fanout.clone();
        }
        merged.unknown.extend(unknown.clone());
        merged
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
