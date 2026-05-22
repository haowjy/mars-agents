use crate::build::bundle::Routing;
use crate::models::availability::resolve_runnable_path;
use crate::models::probes::OpenCodeProbeResult;
use crate::routing::RoutingTrace;

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) route_confidence: String,
    pub(super) provider: Option<&'a str>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub(super) alias_resolution_failed: bool,
    pub(super) route_trace: RoutingTrace,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
    pub(super) warnings: Vec<String>,
}

pub(super) fn resolve_routing(input: RoutingInput<'_>) -> RoutingResolution {
    let RoutingInput {
        model,
        model_token,
        harness,
        route_confidence,
        provider,
        opencode_probe_result,
        alias_resolution_failed,
        route_trace,
    } = input;

    let provider_for_runnable = if alias_resolution_failed {
        ""
    } else {
        provider.unwrap_or("")
    };
    let cached_probe = harness
        .eq_ignore_ascii_case("opencode")
        .then_some(opencode_probe_result)
        .flatten();
    let mut runnable = resolve_runnable_path(&model, provider_for_runnable, &harness, cached_probe);
    if let Some(chosen_slug) = selected_chosen_slug(&route_trace) {
        let use_chosen_slug = harness.eq_ignore_ascii_case("pi")
            || (harness.eq_ignore_ascii_case("opencode")
                && runnable.source == crate::models::availability::RunnablePathSource::Passthrough);
        if use_chosen_slug {
            runnable.harness_model_id = chosen_slug;
            runnable.source = crate::models::availability::RunnablePathSource::CachedProbe;
            runnable.confidence = crate::models::availability::RunnableConfidence::Confirmed;
        }
    }

    RoutingResolution {
        routing: Routing {
            model,
            model_token,
            harness,
            route_confidence,
            harness_model: runnable.harness_model_id,
            harness_model_source: runnable.source.label().to_string(),
            harness_model_confidence: runnable.confidence.label().to_string(),
            route_trace,
        },
        warnings: Vec::new(),
    }
}

fn selected_chosen_slug(trace: &RoutingTrace) -> Option<String> {
    trace
        .assessments
        .iter()
        .find(|assessment| assessment.harness == trace.harness)
        .and_then(|assessment| assessment.chosen_slug.clone())
}
