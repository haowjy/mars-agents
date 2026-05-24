use std::collections::BTreeMap;
use std::path::Path;

use crate::build::bundle::ExecutionPolicy;
use crate::compiler::agents::AgentProfile;
use crate::config::{AgentOverlay, ModelPolicyMatchType, ModelPolicyRule};
use crate::error::{ConfigError, MarsError};
use crate::harness::host::{CapabilityCollectionOptions, collect_capability_snapshot};
use crate::models;

mod config;
mod execution;
mod harness;
mod model;
mod runnable;

pub struct PolicyInput<'a> {
    pub project_root: &'a Path,
    pub agent: Option<&'a str>,
    pub profile: &'a AgentProfile,
    pub model_override: Option<&'a str>,
    pub config_default_model: Option<&'a str>,
    pub harness_override: Option<&'a str>,
    pub effort_override: Option<&'a str>,
    pub approval_override: Option<&'a str>,
    pub sandbox_override: Option<&'a str>,
    pub models_refresh: models::ModelsRefreshControl,
}

pub struct ResolvedPolicy {
    pub routing: crate::build::bundle::Routing,
    pub execution_policy: ExecutionPolicy,
    pub provenance: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PolicySource {
    Cli,
    Overlay,
    OverlayModelPolicy,
    Profile,
    ProfileModelPolicy,
    SettingsModelPolicy,
    Alias,
    Project,
    ConfigOrder,
    Config,
    Provider,
    Default,
    ProfileHarnessOverride,
    Unset,
}

impl PolicySource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Overlay => "overlay",
            Self::OverlayModelPolicy => "overlay-model-policy",
            Self::Profile => "profile",
            Self::ProfileModelPolicy => "profile-model-policy",
            Self::SettingsModelPolicy => "settings-model-policy",
            Self::Alias => "alias",
            Self::Project => "project",
            Self::ConfigOrder => "config-order",
            Self::Config => "config",
            Self::Provider => "provider",
            Self::Default => "default",
            Self::ProfileHarnessOverride => "profile-harness-override",
            Self::Unset => "unset",
        }
    }
}

impl From<crate::routing::RouteSource> for PolicySource {
    fn from(source: crate::routing::RouteSource) -> Self {
        match source {
            crate::routing::RouteSource::Cli => Self::Cli,
            crate::routing::RouteSource::Profile => Self::Profile,
            crate::routing::RouteSource::Alias => Self::Alias,
            crate::routing::RouteSource::ConfigOrder => Self::ConfigOrder,
            crate::routing::RouteSource::ConfigDefault => Self::Config,
            crate::routing::RouteSource::Provider => Self::Provider,
            crate::routing::RouteSource::HardcodedDefault => Self::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PolicyLayer {
    Overlay,
    Profile,
    Settings,
}

impl PolicyLayer {
    fn matched_rule_layer_label(self) -> &'static str {
        match self {
            Self::Overlay => "overlay",
            Self::Profile => "profile",
            Self::Settings => "settings",
        }
    }

    pub(super) fn field_source(self) -> PolicySource {
        match self {
            Self::Overlay => PolicySource::OverlayModelPolicy,
            Self::Profile => PolicySource::ProfileModelPolicy,
            Self::Settings => PolicySource::SettingsModelPolicy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchedPolicyRuleRef {
    pub(super) layer: PolicyLayer,
    pub(super) index: usize,
}

impl MatchedPolicyRuleRef {
    pub(super) fn label(self) -> String {
        format!("{}:{}", self.layer.matched_rule_layer_label(), self.index)
    }
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedField<T> {
    pub(super) value: T,
    pub(super) source: PolicySource,
    pub(super) matched_rule: Option<MatchedPolicyRuleRef>,
}

#[derive(Debug, Clone)]
pub(super) struct MatchedModelPolicy {
    pub(super) layer: PolicyLayer,
    pub(super) index: usize,
    pub(super) rule: ModelPolicyRule,
}

impl MatchedModelPolicy {
    pub(super) fn matched_rule_ref(&self) -> MatchedPolicyRuleRef {
        MatchedPolicyRuleRef {
            layer: self.layer,
            index: self.index,
        }
    }
}

pub fn resolve_policy(input: PolicyInput<'_>) -> Result<ResolvedPolicy, MarsError> {
    let mut warnings = Vec::new();
    let mut provenance = BTreeMap::new();

    let resolution_config = config::load_policy_resolution_config(input.project_root)?;
    let overlay = input
        .agent
        .and_then(|name| resolution_config.agents.get(name));
    let mars_dir = input.project_root.join(".mars");
    let ttl_hours = crate::config::load(input.project_root)
        .map(|config| config.settings.models_cache_ttl_hours)
        .unwrap_or(24);
    let (cache, catalog_outcome) =
        match models::ensure_fresh(&mars_dir, ttl_hours, input.models_refresh.catalog_mode) {
            Ok(pair) => pair,
            Err(err) => {
                warnings.push(format!("models cache unavailable: {err}"));
                (
                    model::load_models_cache(input.project_root).unwrap_or(models::ModelsCache {
                        models: Vec::new(),
                        fetched_at: None,
                    }),
                    models::RefreshOutcome::Offline,
                )
            }
        };
    if let models::RefreshOutcome::StaleFallback { reason } = catalog_outcome {
        warnings.push(format!("models cache: {reason}"));
    }
    let catalog_slugs = models::catalog_model_slugs(&cache);
    let model_input = PolicyInput {
        project_root: input.project_root,
        agent: input.agent,
        profile: input.profile,
        model_override: input.model_override,
        config_default_model: resolution_config.default_model.as_deref(),
        harness_override: input.harness_override,
        effort_override: input.effort_override,
        approval_override: input.approval_override,
        sandbox_override: input.sandbox_override,
        models_refresh: input.models_refresh,
    };
    let resolved_model =
        model::resolve_model(&model_input, overlay, &resolution_config.aliases, &cache)?;

    warnings.extend(resolved_model.warnings);
    provenance.insert(
        "model_source".to_string(),
        resolved_model.model_source.label().to_string(),
    );
    let matched_policy = match_model_policy(
        effective_policies(
            overlay,
            &input.profile.model_policies,
            &resolution_config.settings_model_policies,
        ),
        &resolved_model.model,
        &resolved_model.model_token,
    );

    let capability_snapshot = collect_capability_snapshot(&CapabilityCollectionOptions {
        offline: crate::models::is_mars_offline(),
        probe_refresh: input.models_refresh.probe_refresh,
    });
    let installed_harnesses = capability_snapshot.installed_harnesses();
    let opencode_probe_result = capability_snapshot.opencode.result();
    let pi_probe_result = capability_snapshot.pi.result();
    let cursor_probe_result = capability_snapshot.cursor.result();

    let harness_resolution = harness::resolve_harness(
        &model_input,
        resolved_model.alias,
        overlay,
        matched_policy.as_ref(),
        harness::HarnessEvidence {
            model_id: &resolved_model.model,
            provider_for_order: resolved_model.provider_for_order.as_deref(),
            provider_constraint: resolved_model.provider_constraint.as_deref(),
            settings_provider_order: resolution_config.provider_order.as_deref(),
            config_default_harness: resolution_config.default_harness.as_deref(),
            settings_harness_order: resolution_config.harness_order.as_deref(),
            installed_harnesses: &installed_harnesses,
            linked_harnesses: (!resolution_config.linked_harnesses.is_empty())
                .then_some(resolution_config.linked_harnesses.as_slice()),
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs: Some(catalog_slugs.as_slice()),
        },
    )?;

    warnings.extend(harness_resolution.warnings);
    provenance.insert(
        "harness_source".to_string(),
        harness_resolution.harness.source.label().to_string(),
    );
    provenance.insert(
        "selection_kind".to_string(),
        harness_resolution
            .route_trace
            .selected_selection_kind()
            .label()
            .to_string(),
    );
    provenance.insert(
        "match_evidence".to_string(),
        harness_resolution
            .route_trace
            .selected_match_evidence()
            .label()
            .to_string(),
    );
    provenance.insert(
        "candidates_tried".to_string(),
        harness_resolution.candidates_tried.join(","),
    );
    if harness_resolution.harness.source == PolicySource::ConfigOrder
        && let Some(position) = harness_resolution.harness_order_position
    {
        provenance.insert("harness_order_position".to_string(), position.to_string());
    }
    if harness_resolution.is_experimental {
        warnings.push(
            "Cursor is an experimental launch-bundle target. The contract may change without notice.".to_string(),
        );
        provenance.insert("harness_stability".to_string(), "experimental".to_string());
    }

    let matched_harness_override = input
        .profile
        .harness_overrides
        .get(&harness_resolution.resolved_harness);
    let execution_resolution = execution::resolve_execution_policy(
        &input,
        resolved_model.alias,
        overlay,
        matched_policy.as_ref(),
        matched_harness_override,
    );

    provenance.insert(
        "effort_source".to_string(),
        execution_resolution.effort.source.label().to_string(),
    );
    provenance.insert(
        "approval_source".to_string(),
        execution_resolution.approval.source.label().to_string(),
    );
    provenance.insert(
        "sandbox_source".to_string(),
        execution_resolution.sandbox.source.label().to_string(),
    );
    provenance.insert(
        "autocompact_source".to_string(),
        execution_resolution.autocompact.source.label().to_string(),
    );
    provenance.insert(
        "autocompact_pct_source".to_string(),
        execution_resolution
            .autocompact_pct
            .source
            .label()
            .to_string(),
    );
    if execution_resolution.native_config.is_some() {
        provenance.insert(
            "native_config_source".to_string(),
            PolicySource::ProfileHarnessOverride.label().to_string(),
        );
    }
    let matched_rule = harness_resolution
        .harness
        .matched_rule
        .or(execution_resolution.effort.matched_rule)
        .or(execution_resolution.approval.matched_rule)
        .or(execution_resolution.sandbox.matched_rule)
        .or(execution_resolution.autocompact.matched_rule)
        .or(execution_resolution.autocompact_pct.matched_rule)
        .or_else(|| {
            matched_policy
                .as_ref()
                .map(MatchedModelPolicy::matched_rule_ref)
        });
    if let Some(matched_rule) = matched_rule {
        provenance.insert("matched_policy_rule".to_string(), matched_rule.label());
    }

    let routing_resolution = runnable::resolve_routing(runnable::RoutingInput {
        model: resolved_model.model.clone(),
        model_token: resolved_model.model_token.clone(),
        harness: harness_resolution.harness.value.clone(),
        selection_kind: harness_resolution
            .route_trace
            .selected_selection_kind()
            .label()
            .to_string(),
        match_evidence: harness_resolution
            .route_trace
            .selected_match_evidence()
            .label()
            .to_string(),
        provider_constraint: resolved_model.provider_constraint.as_deref(),
        provider_for_order: resolved_model.provider_for_order.as_deref(),
        settings_provider_order: resolution_config.provider_order.as_deref(),
        effort: execution_resolution.effort.value.clone(),
        opencode_probe_result,
        pi_probe_result,
        cursor_probe_result,
        alias_resolution_failed: resolved_model.alias_resolution_failed,
        route_trace: harness_resolution.route_trace,
    })?;

    let cursor_effort_resolution_failed = routing_resolution
        .warnings
        .iter()
        .any(|warning| warning.contains("no cursor slug matched"));
    warnings.extend(routing_resolution.warnings);

    let mut effort = execution_resolution.effort.value;
    if routing_resolution.effort_consumed {
        effort = None;
        provenance.insert(
            "effort_applied_to_harness_model".to_string(),
            "true".to_string(),
        );
    } else if harness_resolution
        .harness
        .value
        .eq_ignore_ascii_case("cursor")
        && effort
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        && cursor_effort_resolution_failed
    {
        return Err(MarsError::Config(ConfigError::Invalid {
            message: format!(
                "cursor harness cannot resolve model `{}` with effort `{}` from probe catalog",
                resolved_model.model,
                effort.as_deref().unwrap_or_default()
            ),
        }));
    }

    Ok(ResolvedPolicy {
        routing: routing_resolution.routing,
        execution_policy: ExecutionPolicy {
            effort,
            approval: execution_resolution.approval.value,
            sandbox: execution_resolution.sandbox.value,
            autocompact: execution_resolution.autocompact.value,
            autocompact_pct: execution_resolution.autocompact_pct.value,
            timeout: None,
            native_config: execution_resolution.native_config,
            codex_rules: None,
        },
        provenance,
        warnings,
    })
}

pub(super) fn policy_override_string(rule: &ModelPolicyRule, key: &str) -> Option<String> {
    let value = rule
        .overrides
        .get(serde_yaml::Value::String(key.to_string()))?
        .as_str()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(super) fn policy_override_u32(rule: &ModelPolicyRule, key: &str) -> Option<u32> {
    let value = rule
        .overrides
        .get(serde_yaml::Value::String(key.to_string()))?;
    match value {
        serde_yaml::Value::Number(number) => {
            let parsed = number.as_u64()?;
            u32::try_from(parsed).ok()
        }
        _ => None,
    }
}

pub(super) fn policy_override_u8(rule: &ModelPolicyRule, key: &str) -> Option<u8> {
    let value = rule
        .overrides
        .get(serde_yaml::Value::String(key.to_string()))?;
    match value {
        serde_yaml::Value::Number(number) => {
            let parsed = number.as_u64()?;
            let percent = u8::try_from(parsed).ok()?;
            (1..=100).contains(&percent).then_some(percent)
        }
        _ => None,
    }
}

pub(super) fn matched_policy_string_override(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
) -> Option<ResolvedField<String>> {
    let policy = matched_policy?;
    let value = policy_override_string(&policy.rule, key)?;
    Some(ResolvedField {
        value,
        source: policy.layer.field_source(),
        matched_rule: Some(policy.matched_rule_ref()),
    })
}

pub(super) fn matched_policy_u32_override(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
) -> Option<ResolvedField<u32>> {
    let policy = matched_policy?;
    let value = policy_override_u32(&policy.rule, key)?;
    Some(ResolvedField {
        value,
        source: policy.layer.field_source(),
        matched_rule: Some(policy.matched_rule_ref()),
    })
}

pub(super) fn matched_policy_u8_override(
    matched_policy: Option<&MatchedModelPolicy>,
    key: &str,
) -> Option<ResolvedField<u8>> {
    let policy = matched_policy?;
    let value = policy_override_u8(&policy.rule, key)?;
    Some(ResolvedField {
        value,
        source: policy.layer.field_source(),
        matched_rule: Some(policy.matched_rule_ref()),
    })
}

fn effective_policies<'a>(
    overlay: Option<&'a AgentOverlay>,
    profile_policies: &'a [ModelPolicyRule],
    settings_policies: &'a [ModelPolicyRule],
) -> impl Iterator<Item = (PolicyLayer, usize, &'a ModelPolicyRule)> + 'a {
    overlay
        .into_iter()
        .flat_map(|agent_overlay| {
            agent_overlay
                .model_policies
                .iter()
                .enumerate()
                .map(|(index, rule)| (PolicyLayer::Overlay, index, rule))
        })
        .chain(
            profile_policies
                .iter()
                .enumerate()
                .map(|(index, rule)| (PolicyLayer::Profile, index, rule)),
        )
        .chain(
            settings_policies
                .iter()
                .enumerate()
                .map(|(index, rule)| (PolicyLayer::Settings, index, rule)),
        )
}

fn match_model_policy<'a>(
    policies: impl Iterator<Item = (PolicyLayer, usize, &'a ModelPolicyRule)>,
    canonical_model_id: &str,
    selected_model_token: &str,
) -> Option<MatchedModelPolicy> {
    if canonical_model_id.is_empty() || selected_model_token.is_empty() {
        return None;
    }

    for (layer, index, rule) in policies {
        let matched = match rule.match_type {
            ModelPolicyMatchType::Model => rule.match_value == canonical_model_id,
            ModelPolicyMatchType::Alias => rule.match_value == selected_model_token,
            ModelPolicyMatchType::ModelGlob => {
                crate::models::glob_match(&rule.match_value, canonical_model_id)
            }
        };
        if matched {
            return Some(MatchedModelPolicy {
                layer,
                index,
                rule: rule.clone(),
            });
        }
    }

    None
}
