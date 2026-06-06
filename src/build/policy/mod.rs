use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::build::bundle::ExecutionPolicy;
use crate::compiler::agents::AgentProfile;
use crate::config::{AgentOverlay, EffectiveProjectConfig, ModelPolicyMatchType, ModelPolicyRule};
use crate::error::{ConfigError, MarsError};
use crate::harness::host::{CapabilityCollectionOptions, CapabilitySession};
use crate::models::{self, ModelAlias};
use crate::routing;

mod execution;
mod harness;
mod model;
mod runnable;
use runnable::CursorEffortOutcome;

struct SessionProbeResolver<'a> {
    session: &'a mut CapabilitySession,
}

impl crate::routing::ProbeResolver for SessionProbeResolver<'_> {
    fn opencode_probe_result(&mut self) -> Option<crate::models::probes::OpenCodeProbeResult> {
        self.session.opencode_probe_result()
    }

    fn pi_probe_result(&mut self) -> Option<crate::models::probes::PiProbeResult> {
        self.session.pi_probe_result()
    }

    fn cursor_probe_result(&mut self) -> Option<crate::models::probes::CursorProbeResult> {
        self.session.cursor_probe_result()
    }
}

pub struct PolicyInput<'a> {
    pub project_root: &'a Path,
    pub runtime_aliases: &'a IndexMap<String, ModelAlias>,
    pub agent: Option<&'a str>,
    pub profile: &'a AgentProfile,
    pub model_override: Option<&'a str>,
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
            Self::ProfileHarnessOverride => "profile-harness-override",
            Self::Unset => "unset",
        }
    }

    pub(super) fn precedence_rank(self) -> u8 {
        match self {
            Self::Cli => 5,
            Self::Overlay | Self::OverlayModelPolicy => 4,
            Self::Profile | Self::ProfileModelPolicy | Self::ProfileHarnessOverride => 3,
            Self::SettingsModelPolicy | Self::Project | Self::Config => 2,
            Self::Alias => 1,
            Self::Unset | Self::ConfigOrder | Self::Provider => 0,
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

struct ModelFallbackCandidate {
    token: String,
    source: PolicySource,
    match_type: ModelPolicyMatchType,
}

fn is_harness_exhaustion(err: &MarsError) -> bool {
    matches!(
        err,
        MarsError::LinkedHarnessExhausted { .. } | MarsError::HarnessUnavailable { .. }
    )
}

fn selected_alias_token<'a, 'm>(resolved_model: &'a model::ResolvedModel<'m>) -> Option<&'a str> {
    resolved_model
        .alias
        .is_some()
        .then_some(resolved_model.model_token.as_str())
}

pub fn resolve_policy(
    effective_config: &EffectiveProjectConfig,
    input: PolicyInput<'_>,
) -> Result<ResolvedPolicy, MarsError> {
    let mut warnings = Vec::new();
    let mut provenance = BTreeMap::new();

    let aliases = input.runtime_aliases;
    let overlay = input
        .agent
        .and_then(|name| effective_config.agents.get(name));
    let settings_model_policies = &effective_config.settings.model_policies;
    let linked_harnesses = effective_config.settings.linked_harnesses();
    let default_harness_order = crate::harness::registry::default_harness_order_names();
    let harness_order = effective_config
        .settings
        .harness_order
        .as_deref()
        .unwrap_or(default_harness_order.as_slice());
    let mars_dir = input.project_root.join(".mars");
    let ttl_hours = effective_config.settings.models_cache_ttl_hours;
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
    let mut resolved_model = model::resolve_model(
        &input,
        effective_config.settings.default_model.as_deref(),
        overlay,
        aliases,
        &cache,
    )?;

    warnings.extend(resolved_model.warnings.iter().cloned());
    provenance.insert(
        "model_source".to_string(),
        resolved_model.model_source.label().to_string(),
    );
    let primary_model_token = resolved_model.model_token.clone();
    let mut matched_policy = match_model_policy(
        effective_policies(
            overlay,
            &input.profile.model_policies,
            settings_model_policies,
        ),
        &resolved_model.model,
        selected_alias_token(&resolved_model),
    );

    let mut capability_session = CapabilitySession::collect(&CapabilityCollectionOptions {
        offline: crate::models::is_mars_offline(),
        probe_refresh: input.models_refresh.probe_refresh,
    });
    let installed_harnesses = capability_session.installed_harnesses();
    let harness_result = {
        let mut probe_resolver = SessionProbeResolver {
            session: &mut capability_session,
        };
        harness::resolve_harness(
            &input,
            resolved_model.alias,
            overlay,
            matched_policy.as_ref(),
            harness::HarnessEvidence {
                routing: routing::RoutingEvidence {
                    model_id: &resolved_model.model,
                    provider_for_order: resolved_model.provider_for_order.as_deref(),
                    provider_constraint: resolved_model.provider_constraint.as_deref(),
                    settings_provider_order: effective_config.settings.provider_order.as_deref(),
                    config_default_harness: effective_config.settings.default_harness.as_deref(),
                    settings_harness_order: Some(harness_order),
                    installed_harnesses: &installed_harnesses,
                    linked_harnesses: (!linked_harnesses.is_empty())
                        .then_some(linked_harnesses.as_slice()),
                    opencode_probe_result: None,
                    pi_probe_result: None,
                    cursor_probe_result: None,
                    catalog_model_slugs: Some(catalog_slugs.as_slice()),
                },
                model_token: &resolved_model.model_token,
                model_source: resolved_model.model_source,
            },
            &mut probe_resolver,
            crate::models::harness::native_harness_authenticated,
        )
    };
    let mut model_fallback: Option<(String, String)> = None;
    let harness_resolution = match harness_result {
        Ok(resolution) => resolution,
        Err(err) if is_harness_exhaustion(&err) && input.model_override.is_some() => {
            return Err(err);
        }
        Err(err) if is_harness_exhaustion(&err) => {
            let linked_exhaustion = matches!(err, MarsError::LinkedHarnessExhausted { .. });
            let candidates = model_fallback_candidates(
                input.profile,
                &primary_model_token,
                resolved_model.model_source,
                matched_policy.as_ref(),
            );
            let mut exhausted_tokens = Vec::new();
            let mut resolved = None;

            for candidate in candidates {
                let fallback_model = match candidate.match_type {
                    ModelPolicyMatchType::Alias => model::resolve_model_token(
                        candidate.token.clone(),
                        candidate.source,
                        aliases,
                        &cache,
                    )?,
                    ModelPolicyMatchType::Model => {
                        model::resolve_literal_model(candidate.token.clone(), candidate.source)
                    }
                    ModelPolicyMatchType::ModelGlob => unreachable!(
                        "model-glob policies are filtered out of model fallback candidates"
                    ),
                };
                let fallback_policy = match_model_policy(
                    effective_policies(
                        overlay,
                        &input.profile.model_policies,
                        settings_model_policies,
                    ),
                    &fallback_model.model,
                    selected_alias_token(&fallback_model),
                );
                let fallback_result = {
                    let mut probe_resolver = SessionProbeResolver {
                        session: &mut capability_session,
                    };
                    harness::resolve_harness(
                        &input,
                        fallback_model.alias,
                        overlay,
                        fallback_policy.as_ref(),
                        harness::HarnessEvidence {
                            routing: routing::RoutingEvidence {
                                model_id: &fallback_model.model,
                                provider_for_order: fallback_model.provider_for_order.as_deref(),
                                provider_constraint: fallback_model.provider_constraint.as_deref(),
                                settings_provider_order: effective_config
                                    .settings
                                    .provider_order
                                    .as_deref(),
                                config_default_harness: effective_config
                                    .settings
                                    .default_harness
                                    .as_deref(),
                                settings_harness_order: Some(harness_order),
                                installed_harnesses: &installed_harnesses,
                                linked_harnesses: (!linked_harnesses.is_empty())
                                    .then_some(linked_harnesses.as_slice()),
                                opencode_probe_result: None,
                                pi_probe_result: None,
                                cursor_probe_result: None,
                                catalog_model_slugs: Some(catalog_slugs.as_slice()),
                            },
                            model_token: &fallback_model.model_token,
                            model_source: fallback_model.model_source,
                        },
                        &mut probe_resolver,
                        crate::models::harness::native_harness_authenticated,
                    )
                };

                match fallback_result {
                    Ok(harness_resolution) => {
                        warnings.extend(fallback_model.warnings.iter().cloned());
                        warnings.push(format!(
                            "model `{primary_model_token}` unavailable{}; fell back to `{}` on `{}`",
                            if linked_exhaustion {
                                " on linked harnesses"
                            } else {
                                ""
                            },
                            fallback_model.model_token,
                            harness_resolution.harness.value
                        ));
                        provenance.insert(
                            "model_source".to_string(),
                            fallback_model.model_source.label().to_string(),
                        );
                        model_fallback = Some((
                            primary_model_token.clone(),
                            fallback_model.model_token.clone(),
                        ));
                        resolved = Some((fallback_model, fallback_policy, harness_resolution));
                        break;
                    }
                    Err(err) if is_harness_exhaustion(&err) => {
                        exhausted_tokens.push(candidate.token);
                    }
                    Err(err) => return Err(err),
                }
            }

            let Some((fallback_model, fallback_policy, harness_resolution)) = resolved else {
                let mut tried = vec![primary_model_token.clone()];
                tried.extend(exhausted_tokens);
                return Err(MarsError::Config(ConfigError::Invalid {
                    message: format!(
                        "model fallback candidates exhausted for `{}`{}; tried: {}",
                        primary_model_token,
                        if linked_exhaustion {
                            " on linked harnesses"
                        } else {
                            ""
                        },
                        tried.join(", ")
                    ),
                }));
            };

            resolved_model = fallback_model;
            matched_policy = fallback_policy;
            harness_resolution
        }
        Err(err) => return Err(err),
    };

    warnings.extend(harness_resolution.warnings);
    provenance.insert(
        "model_fallback_applied".to_string(),
        model_fallback.is_some().to_string(),
    );
    if let Some((from, to)) = &model_fallback {
        provenance.insert("model_fallback_from".to_string(), from.clone());
        provenance.insert("model_fallback_to".to_string(), to.clone());
    }
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

    let selected_harness = harness_resolution.harness.value.clone();
    let needs_opencode_probe = selected_harness.eq_ignore_ascii_case("opencode");
    let needs_pi_probe = selected_harness.eq_ignore_ascii_case("pi");
    let needs_cursor_probe = selected_harness.eq_ignore_ascii_case("cursor");
    let opencode_probe_result = needs_opencode_probe
        .then(|| capability_session.opencode_probe_result())
        .flatten();
    let pi_probe_result = needs_pi_probe
        .then(|| capability_session.pi_probe_result())
        .flatten();
    let cursor_probe_result = needs_cursor_probe
        .then(|| capability_session.cursor_probe_result())
        .flatten();
    let (
        effective_model,
        effective_model_token,
        effective_provider_constraint,
        effective_provider_for_order,
    ) = if harness_resolution.model_override.is_some() {
        (String::new(), String::new(), None::<String>, None::<String>)
    } else {
        (
            resolved_model.model.clone(),
            resolved_model.model_token.clone(),
            resolved_model.provider_constraint.clone(),
            resolved_model.provider_for_order.clone(),
        )
    };

    let routing_resolution = runnable::resolve_routing(runnable::RoutingInput {
        model: effective_model,
        model_token: effective_model_token,
        harness: selected_harness.clone(),
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
        provider_constraint: effective_provider_constraint.as_deref(),
        provider_for_order: effective_provider_for_order.as_deref(),
        settings_provider_order: effective_config.settings.provider_order.as_deref(),
        effort: execution_resolution.effort.value.clone(),
        opencode_probe_result: opencode_probe_result.as_ref(),
        pi_probe_result: pi_probe_result.as_ref(),
        cursor_probe_result: cursor_probe_result.as_ref(),
        alias_resolution_failed: resolved_model.alias_resolution_failed,
        route_trace: harness_resolution.route_trace,
    })?;

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
        && let Some(cursor_effort) = effort
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        let actor = input
            .agent
            .map(|agent| format!("agent {agent}"))
            .unwrap_or_else(|| "requested model".to_string());
        let message = match routing_resolution.cursor_effort_outcome {
            CursorEffortOutcome::NoEffortVariant => Some(format!(
                "{actor} selected effort {cursor_effort}; Cursor model {} has no {cursor_effort} variant; try --effort medium/none.",
                resolved_model.model
            )),
            CursorEffortOutcome::ProbeUnavailable => Some(format!(
                "{actor} selected effort {cursor_effort}; Cursor model list was unavailable; rerun without --no-refresh-models or with --refresh-models."
            )),
            CursorEffortOutcome::ProbeFailed { error } => {
                let detail = error
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!(" ({value})"))
                    .unwrap_or_default();
                Some(format!(
                    "{actor} selected effort {cursor_effort}; Cursor model list probe failed{detail}."
                ))
            }
            CursorEffortOutcome::ProbeReturnedNoSlugs => Some(format!(
                "{actor} selected effort {cursor_effort}; Cursor model list returned no model slugs; rerun without --no-refresh-models or with --refresh-models."
            )),
            CursorEffortOutcome::NoModelPrefixMatch => Some(format!(
                "{actor} selected effort {cursor_effort}; Cursor model catalog has no matching model slug for `{}`.",
                resolved_model.model
            )),
            CursorEffortOutcome::NotRequested | CursorEffortOutcome::Applied => None,
        };

        if let Some(message) = message {
            return Err(MarsError::Config(ConfigError::Invalid { message }));
        }
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

fn model_fallback_candidates(
    profile: &AgentProfile,
    primary_model_token: &str,
    model_source: PolicySource,
    active_policy: Option<&MatchedModelPolicy>,
) -> Vec<ModelFallbackCandidate> {
    let Some(active_policy) = active_policy else {
        return Vec::new();
    };
    if model_source != PolicySource::Profile
        || active_policy.layer != PolicyLayer::Profile
        || active_policy.rule.no_fallback
    {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    seen.insert(primary_model_token.trim().to_string());

    for policy in profile.model_policies.iter().skip(active_policy.index + 1) {
        if policy.no_fallback {
            continue;
        }
        if !matches!(
            policy.match_type,
            ModelPolicyMatchType::Alias | ModelPolicyMatchType::Model
        ) {
            continue;
        }
        let token = policy.match_value.trim();
        if token.is_empty() || !seen.insert(token.to_string()) {
            continue;
        }
        entries.push(ModelFallbackCandidate {
            token: token.to_string(),
            source: PolicySource::ProfileModelPolicy,
            match_type: policy.match_type.clone(),
        });
    }

    entries
}

fn match_model_policy<'a>(
    policies: impl Iterator<Item = (PolicyLayer, usize, &'a ModelPolicyRule)>,
    canonical_model_id: &str,
    selected_alias_token: Option<&str>,
) -> Option<MatchedModelPolicy> {
    if canonical_model_id.is_empty() {
        return None;
    }

    for (layer, index, rule) in policies {
        let matched = match rule.match_type {
            ModelPolicyMatchType::Model => rule.match_value == canonical_model_id,
            ModelPolicyMatchType::Alias => selected_alias_token == Some(rule.match_value.as_str()),
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
