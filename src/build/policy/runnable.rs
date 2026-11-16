use crate::build::bundle::{CatalogRow, Routing};
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::cursor::{CursorTiers, CursorTiersError, cursor_tiers};
use crate::models::reasoning;
use crate::routing::{MatchEvidence, report::RouteDecisionReport};
use mars_schema::HarnessId;
use mars_schema::agent::{EffortLevel, EffortRequest};
use mars_schema::models::CachedModel;

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) selection_kind: String,
    pub(super) match_evidence: String,
    pub(super) provider_constraint: Option<&'a str>,
    pub(super) provider_for_order: Option<&'a str>,
    /// What the layers ask for; resolved here once the harness model is known.
    pub(super) effort: Option<EffortRequest>,
    /// Every provider's rows, for the model's reasoning and the bundle's `catalog_row`.
    pub(super) catalog: &'a [CachedModel],
    pub(super) cursor_probe_result: Option<&'a CursorProbeResult>,
    pub(super) route_report: RouteDecisionReport,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
    /// The level the harness is launched with; `None` sends no effort, including when Cursor
    /// folded the level into `harness_model`.
    pub(super) effort: Option<EffortLevel>,
    /// Cursor folded the effort into `harness_model`.
    pub(super) effort_consumed: bool,
    pub(super) warnings: Vec<String>,
    /// Cursor could not say which slug carries the requested effort.
    pub(super) cursor_effort_failure: Option<CursorEffortFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CursorEffortFailure {
    ProbeUnavailable,
    ProbeFailed { error: Option<String> },
    ProbeReturnedNoSlugs,
    NoModelPrefixMatch,
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
        catalog,
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

    let catalog_row =
        reasoning::catalog_row(catalog, harness_id, &model, &runnable.harness_model_id)
            .map(CatalogRow::from);
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
        catalog_row,
        route_trace: route_report,
    };
    let mut resolved = RoutingResolution {
        routing: routing.clone(),
        effort: None,
        effort_consumed: false,
        warnings: Vec::new(),
        cursor_effort_failure: None,
    };
    let Some(request @ EffortRequest::Level(_)) = effort else {
        return resolved;
    };

    // Cursor's levels are the slugs its probe lists for this model, so its set comes from the probe.
    let mut cursor_tiers = None;
    let harness_set = if harness_id == HarnessId::Cursor {
        match cursor_tiers_for(&routing.model, cursor_probe_result) {
            Ok(tiers) => {
                let levels = tiers.levels();
                cursor_tiers = Some(tiers);
                levels
            }
            Err(failure) => {
                resolved.cursor_effort_failure = Some(failure);
                reasoning::harness_set(harness_id)
            }
        }
    } else {
        reasoning::harness_set(harness_id)
    };
    let route = reasoning::resolve_for_route(
        request,
        catalog,
        harness_id,
        &routing.model,
        &routing.harness_model,
        &harness_set,
    );
    resolved.warnings.extend(route.warning);
    resolved.effort = route.resolution.level;

    if let (Some(tiers), Some(level)) = (cursor_tiers, resolved.effort)
        && let Some(slug) = tiers.slug_by_level.get(&level)
    {
        routing.harness_model = slug.clone();
        routing.harness_model_source = RunnablePathSource::CachedProbe.label().to_string();
        routing.harness_model_confidence = RunnableConfidence::Confirmed.label().to_string();
        routing.candidate_slugs = tiers.candidate_slugs;
        routing.match_evidence = MatchEvidence::Confirmed.label().to_string();
        resolved.effort = None;
        resolved.effort_consumed = true;
    }
    resolved.routing = routing;
    resolved
}

fn cursor_tiers_for(
    model: &str,
    probe: Option<&CursorProbeResult>,
) -> Result<CursorTiers, CursorEffortFailure> {
    if model.trim().is_empty() {
        return Err(CursorEffortFailure::NoModelPrefixMatch);
    }
    let probe = probe.ok_or(CursorEffortFailure::ProbeUnavailable)?;
    if !probe.model_probe_success {
        return Err(CursorEffortFailure::ProbeFailed {
            error: probe.error.clone(),
        });
    }
    cursor_tiers(model, &probe.slugs).map_err(|error| match error {
        CursorTiersError::NoProbeSlugs => CursorEffortFailure::ProbeReturnedNoSlugs,
        CursorTiersError::NoModelPrefixMatch => CursorEffortFailure::NoModelPrefixMatch,
    })
}
