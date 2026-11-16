use crate::build::bundle::{CatalogRow, Routing};
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::catalog::Catalog;
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::cursor::CursorEffortFailure;
use crate::models::reasoning;
use crate::routing::{MatchEvidence, report::RouteDecisionReport};
use mars_schema::agent::{EffortLevel, EffortRequest};

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
    pub(super) catalog: &'a Catalog,
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

    let route = reasoning::resolve_for_route(
        effort.unwrap_or(EffortRequest::Default),
        catalog,
        harness_id,
        &model,
        &runnable.harness_model_id,
        cursor_probe_result,
    );
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
        catalog_row: route.row.map(CatalogRow::from),
        route_trace: route_report,
    };
    let mut resolved = RoutingResolution {
        routing: routing.clone(),
        effort: route.resolution.level,
        effort_consumed: false,
        warnings: route.warning.iter().map(|w| w.message.clone()).collect(),
        cursor_effort_failure: route.cursor_failure.clone(),
    };
    if route.reasoning_data_missing
        && let Some(EffortRequest::Level(level)) = effort
    {
        resolved.warnings.push(format!(
            "effort `{}` sent as written without checking `{}`: the models catalog has no reasoning data until it refreshes",
            level.as_str(),
            routing.model
        ));
    }
    if let Some(slug) = route.cursor_slug()
        && let Some(tiers) = &route.cursor_tiers
    {
        routing.harness_model = slug.to_string();
        routing.harness_model_source = RunnablePathSource::CachedProbe.label().to_string();
        routing.harness_model_confidence = RunnableConfidence::Confirmed.label().to_string();
        routing.candidate_slugs = tiers.candidate_slugs.clone();
        routing.match_evidence = MatchEvidence::Confirmed.label().to_string();
        resolved.effort = None;
        resolved.effort_consumed = true;
    }
    resolved.routing = routing;
    resolved
}
