use crate::build::policy::{MatchedModelPolicy, PolicyInput, PolicySource, ResolvedField};
use crate::config::AgentOverlay;
use crate::error::{ConfigError, MarsError};
use crate::models::ModelAlias;
use crate::routing;
use mars_schema::HarnessId;

#[derive(Debug)]
pub(super) struct HarnessResolution {
    pub(super) harness: ResolvedField<String>,
    pub(super) harness_order_position: Option<usize>,
    pub(super) candidates_tried: Vec<String>,
    pub(super) route_trace: routing::RoutingTrace,
    pub(super) warnings: Vec<String>,
}

pub(super) enum HarnessAttempt {
    Selected(HarnessResolution),
    Exhausted(routing::RoutingTrace),
}

pub(super) struct HarnessEvidence<'a> {
    pub(super) routing: routing::RoutingEvidence<'a>,
}

pub(super) fn resolve_harness<F>(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    overlay: Option<&AgentOverlay>,
    _matched_policy: Option<&MatchedModelPolicy>,
    evidence: HarnessEvidence<'_>,
    probe_resolver: &mut dyn routing::ProbeResolver,
    auth_check: F,
) -> Result<HarnessAttempt, MarsError>
where
    F: Fn(&str) -> crate::harness::host::AuthState,
{
    let mut warnings = Vec::new();
    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let overlay_harness = overlay
        .and_then(|entry| entry.harness.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut preference = resolve_harness_preference(
        input,
        overlay_harness,
        profile_harness,
        alias.and_then(|entry| entry.harness.as_deref()),
    );
    if let Some(field) = preference.as_mut() {
        field.value = mars_schema::harness::normalize_name(&field.value).ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: format!("invalid {} harness `{}`", field.source.label(), field.value),
            })
        })?;
    }
    let normalized_default = routing::normalize_config_default_harness(
        evidence.routing.config_default_harness,
        &mut warnings,
    );
    let mut routing_input = evidence
        .routing
        .routing_input_with_config_default_harness(normalized_default.as_deref());
    let trace = if let Some(pin) = preference
        .as_ref()
        .filter(|field| field.source == PolicySource::Cli)
    {
        routing_input.provider_for_order = routing::provider_for_order_for_fixed_harness(
            evidence.routing.provider_for_order,
            &pin.value,
        );
        let assessment = routing::evaluate_fixed_harness_with_auth_and_probes(
            &routing_input,
            &pin.value,
            probe_resolver,
            auth_check,
        );
        let rejection = routing::acceptance::accept_assessment(&assessment).err();
        let mut trace = routing::trace_for_fixed_harness(
            routing::RouteSource::Cli,
            &pin.value,
            assessment,
            Vec::new(),
        );
        if let Some(rejection) = rejection {
            trace.diagnostics.push(if rejection.is_not_installed() {
                format!("cli harness `{}` is not installed", pin.value)
            } else {
                format!(
                    "cli harness `{}` cannot run the requested model ({})",
                    pin.value,
                    rejection.skip_reason().unwrap_or("unavailable")
                )
            });
            return Ok(HarnessAttempt::Exhausted(trace));
        }
        trace
    } else {
        routing_input.preferred_harness = preference.as_ref().map(|field| {
            (
                field.value.as_str(),
                route_source_for_policy_source(field.source),
            )
        });
        routing::evaluate_candidates(&routing_input, probe_resolver, auth_check)
    };
    if trace.harness.is_empty() {
        return Ok(HarnessAttempt::Exhausted(trace));
    }
    warnings.extend(trace.selected_diagnostics().iter().cloned());
    let harness = preference
        .filter(|field| field.value == trace.harness)
        .unwrap_or_else(|| ResolvedField {
            value: trace.harness.clone(),
            source: trace.source.into(),
            matched_rule: None,
        });
    Ok(HarnessAttempt::Selected(HarnessResolution {
        harness,
        harness_order_position: trace.harness_order_position,
        candidates_tried: trace.candidates_tried.clone(),
        route_trace: trace,
        warnings,
    }))
}

fn resolve_harness_preference(
    input: &PolicyInput<'_>,
    overlay_harness: Option<&str>,
    profile_harness: Option<&str>,
    alias_harness: Option<&str>,
) -> Option<ResolvedField<String>> {
    if let Some(harness) = input.harness_override {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Cli,
            matched_rule: None,
        });
    }
    if let Some(harness) = overlay_harness {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Overlay,
            matched_rule: None,
        });
    }
    if let Some(harness) = profile_harness {
        return Some(ResolvedField {
            value: harness.to_string(),
            source: PolicySource::Profile,
            matched_rule: None,
        });
    }
    alias_harness.map(|harness| ResolvedField {
        value: harness.to_string(),
        source: PolicySource::Alias,
        matched_rule: None,
    })
}

fn route_source_for_policy_source(source: PolicySource) -> routing::RouteSource {
    match source {
        PolicySource::Cli => routing::RouteSource::Cli,
        PolicySource::Overlay => routing::RouteSource::Overlay,
        PolicySource::OverlayModelPolicy => routing::RouteSource::OverlayModelPolicy,
        PolicySource::Profile => routing::RouteSource::Profile,
        PolicySource::ProfileModelPolicy => routing::RouteSource::ProfileModelPolicy,
        PolicySource::SettingsModelPolicy => routing::RouteSource::SettingsModelPolicy,
        PolicySource::Alias => routing::RouteSource::Alias,
        PolicySource::ConfigOrder => routing::RouteSource::ConfigOrder,
        PolicySource::Config => routing::RouteSource::ConfigDefault,
        _ => routing::RouteSource::Provider,
    }
}

pub(super) fn harness_kind_to_str(harness: &HarnessId) -> &'static str {
    crate::compiler::harness_descriptor::descriptor(*harness).canonical_id
}
