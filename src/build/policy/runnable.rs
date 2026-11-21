use crate::build::bundle::{CatalogRow, Routing};
use crate::harness::registry::{self, SlugShape};
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::catalog::Catalog;
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::Listing;
use crate::models::reasoning;
use crate::models::slug_tiers::{self, Listed};
use crate::routing::report::RouteDecisionReport;
use mars_schema::agent::{EffortLevel, EffortRequest};
use mars_schema::effort::Levels;

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
    /// The harness's listing, read when it folds effort tiers into its slugs.
    pub(super) listing: Option<&'a Listing>,
    /// An explicit effort sits at or above the layer that named the model, so it overrides the
    /// tier the model's own slug carries.
    pub(super) effort_outranks_model: bool,
    pub(super) route_report: RouteDecisionReport,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
    /// The level the harness is launched with; `None` sends no effort, including when the level
    /// is folded into `harness_model`, where it is the tier that slug carries.
    pub(super) effort: Option<EffortLevel>,
    pub(super) warnings: Vec<String>,
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
        listing,
        effort_outranks_model,
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

    let mut runnable = runnable;
    let folds_tiers =
        registry::probe_spec(harness_id).map(|spec| spec.slugs) == Some(SlugShape::Bare);
    let tiers = folds_tiers
        .then_some(listing)
        .flatten()
        .filter(|listing| listing.is_evidence())
        .and_then(|listing| slug_tiers::find(&model, &listing.slugs));
    let mut tier_warnings = Vec::new();
    let route = match tiers {
        Some(found) => {
            let pick = pick_tier(
                found,
                effort,
                effort_outranks_model,
                catalog,
                harness_id,
                &model,
                &mut tier_warnings,
            );
            runnable.harness_model_id = pick.slug;
            runnable.source = RunnablePathSource::CachedProbe;
            runnable.confidence = RunnableConfidence::Confirmed;
            pick.route
        }
        None => {
            let mut route = reasoning::resolve_for_route(
                effort.unwrap_or(EffortRequest::Default),
                catalog,
                harness_id,
                &model,
                &runnable.harness_model_id,
                reasoning::harness_set(harness_id),
            );
            // Without the listing no slug can carry a level, and the harness takes none apart
            // from its slug.
            if folds_tiers && let Some(level) = route.resolution.level.take() {
                tier_warnings.push(format!(
                    "effort `{}` not sent: `{}` is not in {harness_id}'s model list",
                    level.as_str(),
                    runnable.harness_model_id
                ));
            }
            route
        }
    };
    let routing = Routing {
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
    let mut warnings = tier_warnings;
    warnings.extend(route.warning.iter().map(|w| w.message.clone()));
    if route.reasoning_data_missing
        && route.resolution.level.is_some()
        && let Some(EffortRequest::Level(level)) = effort
    {
        warnings.push(format!(
            "effort `{}` sent as written without checking `{}`: the models catalog has no reasoning data until it refreshes",
            level.as_str(),
            routing.model
        ));
    }
    RoutingResolution {
        routing,
        effort: route.resolution.level,
        warnings,
    }
}

struct TierPick<'a> {
    slug: String,
    /// The resolved effort: the level the slug carries, `None` for a bare family or an untiered
    /// slug.
    route: reasoning::RouteEffort<'a>,
}

/// The one slug that carries the model and the effort of a harness that folds tiers into its
/// slugs. An effort the family lacks moves to its nearest tier, never fails; a family with one
/// tier runs it; an untiered slug takes no effort; no effort sends the bare family, which
/// the harness starts at its default tier.
fn pick_tier<'a>(
    found: slug_tiers::Found,
    effort: Option<EffortRequest>,
    effort_outranks_model: bool,
    catalog: &'a Catalog,
    harness: mars_schema::HarnessId,
    model: &str,
    warnings: &mut Vec<String>,
) -> TierPick<'a> {
    let explicit = match effort {
        Some(EffortRequest::Level(level)) => Some(level),
        _ => None,
    };
    let (family, tiers) = match found.listed {
        Listed::Untiered { slug } => {
            if let Some(level) = explicit {
                warnings.push(format!(
                    "effort `{}` not sent: `{slug}` takes no effort",
                    level.as_str()
                ));
            }
            let mut route = reasoning::resolve_for_route(
                EffortRequest::Default,
                catalog,
                harness,
                model,
                &slug,
                reasoning::harness_set(harness),
            );
            route.warning = None;
            return TierPick { slug, route };
        }
        Listed::Family { family, tiers } => (family, tiers),
    };
    let request = match (found.carried, explicit) {
        (Some(carried), Some(level)) if effort_outranks_model => {
            if level != carried {
                warnings.push(format!(
                    "effort `{}` overrides the `{}` tier of `{model}`",
                    level.as_str(),
                    carried.as_str()
                ));
            }
            EffortRequest::Level(level)
        }
        (Some(carried), _) => EffortRequest::Level(carried),
        (None, _) => effort.unwrap_or(EffortRequest::Default),
    };
    let mut route = reasoning::resolve_for_route(
        request,
        catalog,
        harness,
        &family,
        &family,
        Levels::only(tiers.keys().copied()),
    );
    let only_tier = (tiers.len() == 1).then(|| tiers.iter().next()).flatten();
    let slug = match route.resolution.level.and_then(|level| tiers.get(&level)) {
        Some(slug) => slug.clone(),
        None => match only_tier {
            Some((level, slug)) => {
                route.resolution.level = Some(*level);
                slug.clone()
            }
            None => {
                route.resolution.level = None;
                family
            }
        },
    };
    TierPick { slug, route }
}
