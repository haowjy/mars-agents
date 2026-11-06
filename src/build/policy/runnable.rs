use crate::build::bundle::Routing;
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::cursor::{CursorEffortResolutionError, resolve_cursor_effort_slug};
use crate::routing::{MatchEvidence, report::RouteDecisionReport};
use mars_schema::HarnessId;

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) selection_kind: String,
    pub(super) match_evidence: String,
    pub(super) provider_constraint: Option<&'a str>,
    pub(super) provider_for_order: Option<&'a str>,
    pub(super) effort: Option<String>,
    pub(super) cursor_probe_result: Option<&'a CursorProbeResult>,
    pub(super) route_report: RouteDecisionReport,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
    /// Cursor folded the effort into `harness_model`, so the bundle carries none.
    pub(super) effort_consumed: bool,
    pub(super) warnings: Vec<String>,
    pub(super) cursor_effort_outcome: CursorEffortOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CursorEffortOutcome {
    NotRequested,
    Applied,
    ProbeUnavailable,
    ProbeFailed { error: Option<String> },
    ProbeReturnedNoSlugs,
    NoModelPrefixMatch,
    NoEffortVariant,
}

pub(super) fn resolve_routing(input: RoutingInput<'_>) -> RoutingResolution {
    let RoutingInput {
        model,
        model_token,
        harness,
        selection_kind,
        match_evidence,
        provider_constraint,
        provider_for_order,
        effort,
        cursor_probe_result,
        route_report,
    } = input;

    let selected_assessment = route_report
        .selected_attempt()
        .into_iter()
        .flat_map(|attempt| &attempt.assessments)
        .find(|assessment| assessment.harness == harness);
    let harness_id = mars_schema::harness::parse(&harness).expect("selected harness is registered");
    let runnable = resolve_harness_model(
        harness_id,
        &model,
        selected_assessment.and_then(|assessment| assessment.chosen_slug.as_deref()),
        selected_assessment.and_then(|assessment| assessment.chosen_model.as_deref()),
        provider_constraint,
        provider_for_order,
    );
    let candidate_slugs = selected_assessment
        .map(|assessment| assessment.candidate_slugs.clone())
        .unwrap_or_default();

    let mut routing = Routing {
        model,
        model_token,
        provider_constraint: provider_constraint.map(str::to_string),
        harness: harness.clone(),
        selection_kind,
        match_evidence,
        harness_model: runnable.harness_model_id,
        harness_model_source: runnable.source.label().to_string(),
        harness_model_confidence: runnable.confidence.label().to_string(),
        candidate_slugs,
        route_trace: route_report,
    };
    let mut effort_consumed = false;
    let mut cursor_effort_outcome = CursorEffortOutcome::NotRequested;
    let mut warnings = Vec::new();

    if harness_id == HarnessId::Cursor
        && !routing.model.trim().is_empty()
        && let Some(requested) = effort.clone().filter(|value| !value.trim().is_empty())
    {
        cursor_effort_outcome = CursorEffortOutcome::ProbeUnavailable;
        match cursor_probe_result {
            Some(probe) if !probe.model_probe_success => {
                cursor_effort_outcome = CursorEffortOutcome::ProbeFailed {
                    error: probe.error.clone(),
                };
            }
            Some(probe) => {
                if probe.slugs.is_empty() {
                    cursor_effort_outcome = CursorEffortOutcome::ProbeReturnedNoSlugs;
                } else {
                    match resolve_cursor_effort_slug(&routing.model, &requested, &probe.slugs) {
                        Ok(resolution) => {
                            if let Some(tier) = resolution.max_approximated_by {
                                warnings.push(format!(
                                    "effort: Cursor model {} has no max variant; using {} ({tier})",
                                    routing.model, resolution.slug
                                ));
                            }
                            routing.harness_model = resolution.slug;
                            routing.harness_model_source =
                                RunnablePathSource::CachedProbe.label().to_string();
                            routing.harness_model_confidence =
                                RunnableConfidence::Confirmed.label().to_string();
                            routing.candidate_slugs = resolution.candidate_slugs;
                            routing.match_evidence = MatchEvidence::Confirmed.label().to_string();
                            effort_consumed = true;
                            cursor_effort_outcome = CursorEffortOutcome::Applied;
                        }
                        Err(CursorEffortResolutionError::NoEffortMatch { .. }) => {
                            cursor_effort_outcome = CursorEffortOutcome::NoEffortVariant;
                        }
                        Err(CursorEffortResolutionError::NoModelPrefixMatch) => {
                            cursor_effort_outcome = CursorEffortOutcome::NoModelPrefixMatch;
                        }
                        Err(CursorEffortResolutionError::NoProbeSlugs) => {
                            cursor_effort_outcome = CursorEffortOutcome::ProbeReturnedNoSlugs;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    RoutingResolution {
        routing,
        effort_consumed,
        warnings,
        cursor_effort_outcome,
    }
}
