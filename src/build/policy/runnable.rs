use crate::build::bundle::Routing;
use crate::models::availability::resolve_runnable_path;
use crate::models::probes::OpenCodeProbeResult;

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) route_confidence: String,
    pub(super) provider: Option<&'a str>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub(super) alias_resolution_failed: bool,
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
    let runnable = resolve_runnable_path(&model, provider_for_runnable, &harness, cached_probe);

    RoutingResolution {
        routing: Routing {
            model,
            model_token,
            harness,
            route_confidence,
            harness_model: runnable.harness_model_id,
            harness_model_source: runnable.source.label().to_string(),
            harness_model_confidence: runnable.confidence.label().to_string(),
        },
        warnings: Vec::new(),
    }
}
