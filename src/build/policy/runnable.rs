use crate::build::bundle::{CatalogRow, Routing};
use crate::harness::registry::{self, SlugShape};
use crate::models::catalog::Catalog;
use crate::models::harness_model::resolve_harness_model;
use crate::models::reasoning;
use crate::models::slug_tiers::{self, Listed};
use crate::routing::MatchEvidence;
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
    /// How the effort ranks against the layer that named the model, for the tier a tiered `-m`
    /// slug carries.
    pub(super) effort_standing: EffortStanding,
    pub(super) route_report: RouteDecisionReport,
}

/// An explicit effort against the tier its model's own slug carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffortStanding {
    /// At or above the layer that named the model: it overrides the tier.
    Outranks,
    /// Below that layer: the slug's tier wins, with a warning.
    Yields,
    /// No explicit level, or a default (the alias's, the settings'): the slug's tier wins quietly.
    Implied,
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
        effort_standing,
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
    // The assessment's candidates are what `slug_tiers::find` found in the listing.
    let tiers = folds_tiers
        .then(|| slug_tiers::find(&model, &candidate_slugs))
        .flatten();
    let mut tier_warnings = Vec::new();
    let route = match tiers {
        Some(found) => {
            let pick = pick_tier(
                found,
                effort,
                effort_standing,
                catalog,
                harness_id,
                &model,
                &mut tier_warnings,
            );
            runnable.harness_model_id = pick.slug;
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
            // Without a listed slug the harness has nothing to carry a level in, and it takes
            // none apart from its slug.
            if folds_tiers && let Some(level) = route.resolution.level.take() {
                let level = level.as_str();
                tier_warnings.push(if model.is_empty() {
                    format!("effort `{level}` not sent: no model is set, so {harness_id} has no slug to carry it")
                } else if match_evidence == MatchEvidence::Passthrough.label() {
                    format!("effort `{level}` not sent: there is no {harness_id} model list to find a slug for `{model}` in")
                } else {
                    format!("effort `{level}` not sent: `{model}` is not in {harness_id}'s model list")
                });
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
        && let Some(EffortRequest::Level(level)) = effort
        && route.resolution.level == Some(level)
    {
        let checked = if routing.model.is_empty() {
            "no model is set".to_string()
        } else {
            format!("`{}`", routing.model)
        };
        warnings.push(format!(
            "effort `{}` sent as written without checking {checked}: the models catalog has no reasoning data until it refreshes",
            level.as_str()
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
    standing: EffortStanding,
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
        (Some(_), Some(level)) if standing == EffortStanding::Outranks => {
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
    if let (Some(carried), Some(level)) = (found.carried, explicit)
        && level != carried
    {
        match standing {
            // A move already says what became of the effort.
            EffortStanding::Outranks
                if route.warning.is_none() && route.resolution.level != Some(carried) =>
            {
                warnings.push(format!(
                    "effort `{}` overrides the `{}` tier of `{model}`",
                    level.as_str(),
                    carried.as_str()
                ));
            }
            EffortStanding::Yields => warnings.push(format!(
                "effort `{}` ignored: `{model}` names its `{}` tier at a layer that outranks the effort",
                level.as_str(),
                carried.as_str()
            )),
            _ => {}
        }
    }
    TierPick { slug, route }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EffortLevel::{High, Low, Max, Medium};
    use EffortStanding::{Implied, Outranks, Yields};
    use mars_schema::HarnessId;

    const SLUGS: [&str; 8] = [
        "gemini-3.8-flash-high",
        "gemini-3.8-flash-low",
        "gpt-oss-120b-medium",
        "gemini-pro-orchestrator-neon",
        "gemini-4-neon-high",
        "gemini-4-argon-high",
        "gemini-3.8-pro-low",
        "gemini-3.8-pro-medium",
    ];

    /// The slug sent, the effort it carries, and the warnings, for `model` and `effort`.
    fn pick(
        model: &str,
        effort: Option<EffortLevel>,
        standing: EffortStanding,
    ) -> Option<(String, Option<EffortLevel>, usize)> {
        let slugs = SLUGS.map(String::from);
        let found = slug_tiers::find(model, &slugs)?;
        let catalog = Catalog::default();
        let mut warnings = Vec::new();
        let pick = pick_tier(
            found,
            effort.map(EffortRequest::Level),
            standing,
            &catalog,
            HarnessId::Agy,
            model,
            &mut warnings,
        );
        let warned = warnings.len() + usize::from(pick.route.warning.is_some());
        Some((pick.slug, pick.route.resolution.level, warned))
    }

    #[test]
    fn one_slug_carries_the_model_and_the_effort() {
        let s = |slug: &str, level, warned| Some((slug.to_string(), level, warned));
        // A family: the effort picks the tier, a missing one moves to the nearest.
        assert_eq!(
            pick("gemini-3.8-flash", Some(High), Implied),
            s("gemini-3.8-flash-high", Some(High), 0)
        );
        assert_eq!(
            pick("gemini-3.8-flash", Some(Max), Implied),
            s("gemini-3.8-flash-high", Some(High), 1)
        );
        // No effort sends the bare family; a one-tier family runs its tier.
        assert_eq!(
            pick("gemini-3.8-flash", None, Implied),
            s("gemini-3.8-flash", None, 0)
        );
        assert_eq!(
            pick("gpt-oss-120b", None, Implied),
            s("gpt-oss-120b-medium", Some(Medium), 0)
        );
        // An untiered slug takes no effort, and says so.
        assert_eq!(
            pick("gemini-pro-orchestrator-neon", Some(Low), Implied),
            s("gemini-pro-orchestrator-neon", None, 1)
        );
        // A tiered `-m` keeps its tier, or yields to an effort at its layer or above: one warning.
        assert_eq!(
            pick("gemini-3.8-flash-low", None, Implied),
            s("gemini-3.8-flash-low", Some(Low), 0)
        );
        assert_eq!(
            pick("gemini-3.8-flash-low", Some(High), Outranks),
            s("gemini-3.8-flash-high", Some(High), 1)
        );
        assert_eq!(
            pick("gemini-3.8-flash-low", Some(Low), Outranks),
            s("gemini-3.8-flash-low", Some(Low), 0)
        );
        assert_eq!(
            pick("gemini-3.8-flash-low", Some(Max), Outranks),
            s("gemini-3.8-flash-high", Some(High), 1)
        );
        assert_eq!(
            pick("gpt-oss-120b-medium", Some(High), Outranks),
            s("gpt-oss-120b-medium", Some(Medium), 1)
        );
        // An effort below the layer that named the model is told it lost; an alias default is not.
        assert_eq!(
            pick("gemini-3.8-flash-low", Some(High), Yields),
            s("gemini-3.8-flash-low", Some(Low), 1)
        );
        assert_eq!(
            pick("gemini-3.8-flash-low", Some(High), Implied),
            s("gemini-3.8-flash-low", Some(Low), 0)
        );
    }

    #[test]
    fn a_prefix_shared_by_several_families_is_not_a_model() {
        assert_eq!(pick("gemini-4", Some(High), Outranks), None);
        assert_eq!(pick("gemini-pro-orchestrator", None, Implied), None);
    }
}
