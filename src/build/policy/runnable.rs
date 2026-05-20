use crate::build::bundle::Routing;
use crate::models::ModelsCache;
use crate::models::availability::{RunnableConfidence, RunnablePathSource, resolve_runnable_path};
use crate::models::probes::OpenCodeProbeResult;

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) harness_source: &'static str,
    pub(super) route_confidence: String,
    pub(super) provider: Option<&'a str>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub(super) alias_resolution_failed: bool,
    pub(super) alias_exists: bool,
    pub(super) cache: &'a ModelsCache,
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
        harness_source,
        route_confidence,
        provider,
        opencode_probe_result,
        alias_resolution_failed,
        alias_exists,
        cache,
    } = input;

    let mut warnings = Vec::new();

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
    let fixed_harness_selection = matches!(harness_source, "cli" | "profile");

    if !fixed_harness_selection
        && matches!(
            runnable.source,
            RunnablePathSource::Synthesized | RunnablePathSource::Passthrough
        )
    {
        warnings.push(format!(
            "model '{}' does not have a confirmed runnable path for harness '{}'; using {} path '{}'",
            model,
            harness,
            runnable.source.label(),
            runnable.harness_model_id
        ));
    }
    if !fixed_harness_selection && runnable.confidence == RunnableConfidence::Unknown {
        warnings.push(format!(
            "harness-model for '{}' targeting '{}' is unconfirmed ({})",
            model,
            harness,
            runnable.source.label()
        ));
    }
    if !fixed_harness_selection
        && !alias_exists
        && model_token == model
        && !model_exists_in_cache(cache, &model)
        && matches!(runnable.source, RunnablePathSource::Passthrough)
    {
        warnings.push(format!(
            "model '{}' not found in models cache; passing through as harness model ID",
            model_token
        ));
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
        },
        warnings,
    }
}

fn model_exists_in_cache(cache: &ModelsCache, model_id: &str) -> bool {
    cache
        .models
        .iter()
        .any(|model| model.id.eq_ignore_ascii_case(model_id))
}
