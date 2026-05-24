use crate::build::bundle::Routing;
use crate::error::{ConfigError, MarsError};
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::harness_model::{
    HarnessModelInput, pi_harness_model_requires_probe_slug, resolve_harness_model,
};
use crate::models::probes::cursor::{CursorEffortResolutionError, resolve_cursor_effort_slug};
use crate::models::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};
use crate::routing::{MatchEvidence, RoutingTrace};

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) selection_kind: String,
    pub(super) match_evidence: String,
    pub(super) provider_constraint: Option<&'a str>,
    pub(super) provider_for_order: Option<&'a str>,
    pub(super) settings_provider_order: Option<&'a [String]>,
    pub(super) effort: Option<String>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub(super) pi_probe_result: Option<&'a PiProbeResult>,
    pub(super) cursor_probe_result: Option<&'a CursorProbeResult>,
    pub(super) alias_resolution_failed: bool,
    pub(super) route_trace: RoutingTrace,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
    pub(super) warnings: Vec<String>,
    pub(super) effort_consumed: bool,
}

pub(super) fn resolve_routing(input: RoutingInput<'_>) -> Result<RoutingResolution, MarsError> {
    let RoutingInput {
        model,
        model_token,
        harness,
        selection_kind,
        match_evidence,
        provider_constraint,
        provider_for_order,
        settings_provider_order,
        effort,
        opencode_probe_result,
        pi_probe_result,
        cursor_probe_result,
        alias_resolution_failed,
        route_trace,
    } = input;

    let effective_provider_constraint = if alias_resolution_failed {
        None
    } else {
        provider_constraint
    };
    let effective_provider_for_order = if alias_resolution_failed {
        None
    } else {
        provider_for_order
    };

    let runnable = resolve_harness_model(HarnessModelInput {
        harness: &harness,
        model_id: &model,
        provider_constraint: effective_provider_constraint,
        provider_for_order: effective_provider_for_order,
        settings_provider_order,
        opencode_probe: opencode_probe_result,
        pi_probe: pi_probe_result,
    });

    if pi_harness_model_requires_probe_slug(&harness, &selection_kind, &model, &runnable) {
        return Err(fixed_pi_harness_model_error(&harness));
    }

    let candidate_slugs = route_trace
        .assessments
        .iter()
        .find(|assessment| assessment.harness == harness)
        .map(|assessment| assessment.candidate_slugs.clone())
        .unwrap_or_default();

    let mut routing = Routing {
        model,
        model_token,
        harness: harness.clone(),
        selection_kind,
        match_evidence,
        harness_model: runnable.harness_model_id,
        harness_model_source: runnable.source.label().to_string(),
        harness_model_confidence: runnable.confidence.label().to_string(),
        candidate_slugs,
        route_trace: route_trace.to_report(),
    };
    let mut warnings = Vec::new();
    let mut effort_consumed = false;

    if harness.eq_ignore_ascii_case("cursor")
        && let Some(effort) = effort.filter(|value| !value.trim().is_empty())
    {
        match cursor_probe_result {
            Some(probe) if probe.model_probe_success && !probe.slugs.is_empty() => {
                match resolve_cursor_effort_slug(&routing.model, &effort, &probe.slugs) {
                    Ok(resolution) => {
                        routing.harness_model = resolution.slug;
                        routing.harness_model_source =
                            RunnablePathSource::CachedProbe.label().to_string();
                        routing.harness_model_confidence =
                            RunnableConfidence::Confirmed.label().to_string();
                        routing.candidate_slugs = resolution.candidate_slugs;
                        routing.match_evidence = MatchEvidence::Confirmed.label().to_string();
                        effort_consumed = true;
                        warnings.push(format!(
                            "applied effort `{effort}` to cursor harness_model `{}`",
                            routing.harness_model
                        ));
                    }
                    Err(CursorEffortResolutionError::NoEffortMatch { .. }) => {
                        warnings.push(format!(
                            "no cursor slug matched model `{}` with effort `{effort}`",
                            routing.model
                        ));
                    }
                    Err(CursorEffortResolutionError::NoModelPrefixMatch) => {
                        warnings.push(format!(
                            "cursor probe has no slug matching model `{}` for effort `{effort}`",
                            routing.model
                        ));
                    }
                    Err(CursorEffortResolutionError::NoProbeSlugs) => {
                        warnings.push(
                            "cursor effort resolution requested but probe returned no slugs"
                                .to_string(),
                        );
                    }
                }
            }
            _ => {
                warnings.push(
                    "cursor effort resolution requested but cursor probe is unavailable"
                        .to_string(),
                );
            }
        }
    }

    Ok(RoutingResolution {
        routing,
        warnings,
        effort_consumed,
    })
}

fn fixed_pi_harness_model_error(harness: &str) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!(
            "cli harness `{harness}` cannot run requested model under model-first routing (no_model_match)"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use crate::routing::SelectionKind;

    fn trace_with_assessment(evidence: MatchEvidence) -> RoutingTrace {
        RoutingTrace {
            source: crate::routing::RouteSource::Provider,
            selection_kind: SelectionKind::Auto,
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
    fn opencode_uses_probe_slug_when_probe_available() {
        let opencode_probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4-mini".to_string()],
            model_probe_success: true,
            error: None,
        };
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gptmini".to_string(),
            harness: "opencode".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            effort: None,
            opencode_probe_result: Some(&opencode_probe),
            pi_probe_result: None,
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.routing.harness_model,
            "openai/gpt-5.4-mini".to_string()
        );
        assert_eq!(
            resolution.routing.harness_model_source,
            "cached-probe".to_string()
        );
    }

    #[test]
    fn opencode_keeps_passthrough_model_when_probe_unavailable() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gptmini".to_string(),
            harness: "opencode".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "passthrough".to_string(),
            provider_constraint: None,
            provider_for_order: None,
            settings_provider_order: None,
            effort: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Passthrough),
        })
        .expect("routing should resolve");

        assert_eq!(resolution.routing.harness_model, "gpt-5.4-mini".to_string());
    }

    #[test]
    fn pi_fixed_without_probe_slug_errors() {
        let trace = RoutingTrace {
            source: crate::routing::RouteSource::Cli,
            selection_kind: SelectionKind::Fixed,
            match_evidence: MatchEvidence::Passthrough,
            harness: "pi".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["pi".to_string()],
            assessments: vec![crate::routing::CandidateAssessment {
                harness: "pi".to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: Some(MatchEvidence::Passthrough),
                skip_reason: None,
            }],
            diagnostics: Vec::new(),
        };
        let err = match resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gpt-5.4-mini".to_string(),
            harness: "pi".to_string(),
            selection_kind: "fixed".to_string(),
            match_evidence: "passthrough".to_string(),
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            effort: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace,
        }) {
            Err(err) => err,
            Ok(_) => panic!("fixed pi without probe slug should fail"),
        };

        assert!(err.to_string().contains("no_model_match"));
    }

    #[test]
    fn pi_uses_probe_slug_for_bare_model() {
        let mut model_slugs = HashSet::new();
        model_slugs.insert("openai-codex/gpt-5.4-mini".to_string());
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs,
            ..PiProbeResult::default()
        };
        let trace = RoutingTrace {
            source: crate::routing::RouteSource::Cli,
            selection_kind: SelectionKind::Fixed,
            match_evidence: MatchEvidence::Confirmed,
            harness: "pi".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["pi".to_string()],
            assessments: vec![crate::routing::CandidateAssessment {
                harness: "pi".to_string(),
                installed: true,
                candidate_slugs: vec!["openai-codex/gpt-5.4-mini".to_string()],
                filtered_slugs: vec!["openai-codex/gpt-5.4-mini".to_string()],
                chosen_slug: Some("openai-codex/gpt-5.4-mini".to_string()),
                chosen_model: Some("gpt-5.4-mini".to_string()),
                match_evidence: Some(MatchEvidence::Confirmed),
                skip_reason: None,
            }],
            diagnostics: Vec::new(),
        };
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gpt-5.4-mini".to_string(),
            harness: "pi".to_string(),
            selection_kind: "fixed".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: Some("openai"),
            settings_provider_order: None,
            effort: None,
            opencode_probe_result: None,
            pi_probe_result: Some(&pi_probe),
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace,
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.routing.harness_model,
            "openai-codex/gpt-5.4-mini".to_string()
        );
        assert_eq!(
            resolution.routing.harness_model_source,
            "cached-probe".to_string()
        );
    }

    #[test]
    fn cursor_applies_effort_to_harness_model() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.5".to_string(),
            model_token: "gpt-5.5".to_string(),
            harness: "cursor".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: None,
            settings_provider_order: None,
            effort: Some("high".to_string()),
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert!(resolution.effort_consumed);
        assert_eq!(resolution.routing.harness_model, "gpt-5.5-high");
        assert_eq!(resolution.routing.harness_model_confidence, "confirmed");
    }

    #[test]
    fn cursor_applies_medium_effort_to_unsuffixed_harness_model() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.5".to_string(),
            model_token: "gpt-5.5".to_string(),
            harness: "cursor".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: None,
            settings_provider_order: None,
            effort: Some("medium".to_string()),
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec![
                    "gpt-5.5".to_string(),
                    "gpt-5.5-high".to_string(),
                    "gpt-5.5-low".to_string(),
                ],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert!(resolution.effort_consumed);
        assert_eq!(resolution.routing.harness_model, "gpt-5.5");
    }

    #[test]
    fn empty_model_keeps_empty_harness_model_for_harness_default() {
        let resolution = resolve_routing(RoutingInput {
            model: String::new(),
            model_token: String::new(),
            harness: "claude".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "passthrough".to_string(),
            provider_constraint: None,
            provider_for_order: None,
            settings_provider_order: None,
            effort: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: RoutingTrace {
                source: crate::routing::RouteSource::Provider,
                selection_kind: SelectionKind::Auto,
                match_evidence: MatchEvidence::Passthrough,
                harness: "claude".to_string(),
                harness_order_position: None,
                candidates_tried: vec!["claude".to_string()],
                assessments: Vec::new(),
                diagnostics: Vec::new(),
            },
        })
        .expect("routing should resolve");

        assert_eq!(resolution.routing.model, "");
        assert_eq!(resolution.routing.model_token, "");
        assert_eq!(resolution.routing.harness_model, "");
        assert_eq!(
            resolution.routing.harness_model_source,
            "passthrough".to_string()
        );
    }
}
