use crate::build::bundle::Routing;
use crate::models::availability::resolve_runnable_path;
use crate::models::probes::OpenCodeProbeResult;
use crate::routing::{MatchEvidence, RoutingTrace};

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) selection_kind: String,
    pub(super) match_evidence: String,
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
        selection_kind,
        match_evidence,
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
    if let Some(chosen_slug) = route_trace.selected_chosen_slug_evidence() {
        let use_chosen_slug = harness.eq_ignore_ascii_case("pi")
            || (harness.eq_ignore_ascii_case("opencode")
                && matches!(
                    chosen_slug.match_evidence,
                    Some(MatchEvidence::Confirmed | MatchEvidence::Constrained)
                ));
        if use_chosen_slug {
            runnable.harness_model_id = chosen_slug.slug;
            runnable.source = crate::models::availability::RunnablePathSource::CachedProbe;
            runnable.confidence = crate::models::availability::RunnableConfidence::Confirmed;
        }
    }

    RoutingResolution {
        routing: Routing {
            model,
            model_token,
            harness,
            selection_kind,
            match_evidence,
            harness_model: runnable.harness_model_id,
            harness_model_source: runnable.source.label().to_string(),
            harness_model_confidence: runnable.confidence.label().to_string(),
            route_trace: route_trace.to_report(),
        },
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_with_assessment(evidence: MatchEvidence) -> RoutingTrace {
        RoutingTrace {
            source: crate::routing::RouteSource::Provider,
            selection_kind: crate::routing::SelectionKind::Auto,
            match_evidence: evidence,
            harness: "opencode".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["opencode".to_string()],
            assessments: vec![crate::routing::CandidateAssessment {
                harness: "opencode".to_string(),
                installed: true,
                candidate_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                filtered_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                chosen_slug: Some("openai/gpt-5.4-mini".to_string()),
                chosen_model: Some("gpt-5.4-mini".to_string()),
                match_evidence: Some(evidence),
                skip_reason: None,
            }],
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn opencode_uses_chosen_slug_when_assessment_evidence_is_confirmed() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gptmini".to_string(),
            harness: "opencode".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "confirmed".to_string(),
            provider: None,
            opencode_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        });

        assert_eq!(
            resolution.routing.harness_model,
            "openai/gpt-5.4-mini".to_string()
        );
    }

    #[test]
    fn opencode_keeps_passthrough_model_when_assessment_evidence_is_passthrough() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gptmini".to_string(),
            harness: "opencode".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "passthrough".to_string(),
            provider: None,
            opencode_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Passthrough),
        });

        assert_eq!(resolution.routing.harness_model, "gpt-5.4-mini".to_string());
    }
}
