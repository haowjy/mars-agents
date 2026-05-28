use crate::build::bundle::Routing;
use crate::error::MarsError;
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::harness_model::{HarnessModelInput, resolve_harness_model};
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

    // When an explicit harness (fixed selection) can't probe-match the model,
    // clear the model and warn instead of hard-erroring. The harness will
    // use its own default model.
    let mut model = model;
    let mut model_token = model_token;
    let mut warnings: Vec<String> = Vec::new();

    let unmatched_fixed_harness = harness.eq_ignore_ascii_case("pi")
        && selection_kind.eq_ignore_ascii_case("fixed")
        && !model.trim().is_empty()
        && runnable.source == RunnablePathSource::Passthrough
        && !runnable.harness_model_id.contains('/');

    if unmatched_fixed_harness {
        warnings.push(format!(
            "explicit harness `{harness}` could not match model `{model}`; \
             clearing model so {harness} uses its default"
        ));
        model = String::new();
        model_token = String::new();
    }

    let candidate_slugs = route_trace
        .assessments
        .iter()
        .find(|assessment| assessment.harness == harness)
        .map(|assessment| assessment.candidate_slugs.clone())
        .unwrap_or_default();

    let harness_model = if unmatched_fixed_harness {
        String::new()
    } else {
        runnable.harness_model_id
    };

    let mut routing = Routing {
        model,
        model_token,
        harness: harness.clone(),
        selection_kind,
        match_evidence,
        harness_model,
        harness_model_source: runnable.source.label().to_string(),
        harness_model_confidence: runnable.confidence.label().to_string(),
        candidate_slugs,
        route_trace: route_trace.to_report(),
    };
    let mut effort_consumed = false;
    let mut cursor_effort_outcome = CursorEffortOutcome::NotRequested;

    if harness.eq_ignore_ascii_case("cursor")
        && !routing.model.trim().is_empty()
        && let Some(effort) = effort.filter(|value| !value.trim().is_empty())
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

    Ok(RoutingResolution {
        routing,
        warnings,
        effort_consumed,
        cursor_effort_outcome,
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
    fn pi_fixed_without_probe_slug_clears_model_and_warns() {
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
        let result = resolve_routing(RoutingInput {
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
        })
        .expect("fixed pi without probe slug should not error");

        assert_eq!(result.routing.harness, "pi");
        assert!(
            result.routing.model.is_empty(),
            "model should be cleared when probe can't match"
        );
        assert!(
            result.routing.harness_model.is_empty(),
            "harness_model should be cleared when probe can't match"
        );
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("could not match model"));
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
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::Applied
        );
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
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::Applied
        );
        assert_eq!(resolution.routing.harness_model, "gpt-5.5");
    }

    #[test]
    fn cursor_applies_effort_to_composer_bare_slug_when_variant_missing() {
        let resolution = resolve_routing(RoutingInput {
            model: "composer-2.5".to_string(),
            model_token: "composer-2.5".to_string(),
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
                slugs: vec!["composer-2.5".to_string(), "composer-2.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert!(resolution.effort_consumed);
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::Applied
        );
        assert_eq!(resolution.routing.harness_model, "composer-2.5");
    }

    #[test]
    fn cursor_prefers_exact_effort_variant_for_composer_when_available() {
        let resolution = resolve_routing(RoutingInput {
            model: "composer-2.5".to_string(),
            model_token: "composer-2.5".to_string(),
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
                slugs: vec![
                    "composer-2.5".to_string(),
                    "composer-2.5-high".to_string(),
                    "composer-2.5-low".to_string(),
                ],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert!(resolution.effort_consumed);
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::Applied
        );
        assert_eq!(resolution.routing.harness_model, "composer-2.5-high");
    }

    #[test]
    fn cursor_non_composer_bare_slug_without_variant_reports_missing_effort_variant() {
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
                slugs: vec!["gpt-5.5".to_string(), "gpt-5.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert!(!resolution.effort_consumed);
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::NoEffortVariant
        );
        assert_eq!(resolution.routing.harness_model, "gpt-5.5");
    }

    #[test]
    fn cursor_probe_unavailable_reports_typed_outcome() {
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
            cursor_probe_result: None,
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::ProbeUnavailable
        );
    }

    #[test]
    fn cursor_probe_empty_slugs_reports_typed_outcome() {
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
                slugs: Vec::new(),
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::ProbeReturnedNoSlugs
        );
    }

    #[test]
    fn cursor_probe_failure_reports_typed_outcome_with_error() {
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
                slugs: Vec::new(),
                model_probe_success: false,
                error: Some("model probe failed: timeout".to_string()),
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::ProbeFailed {
                error: Some("model probe failed: timeout".to_string())
            }
        );
    }

    #[test]
    fn cursor_probe_no_prefix_match_reports_typed_outcome() {
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
                slugs: vec!["claude-opus-4-7-high".to_string()],
                model_probe_success: true,
                error: None,
            }),
            alias_resolution_failed: false,
            route_trace: trace_with_assessment(MatchEvidence::Confirmed),
        })
        .expect("routing should resolve");

        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::NoModelPrefixMatch
        );
    }

    #[test]
    fn cursor_effort_with_empty_model_skips_slug_resolution() {
        let resolution = resolve_routing(RoutingInput {
            model: String::new(),
            model_token: String::new(),
            harness: "cursor".to_string(),
            selection_kind: "fixed".to_string(),
            match_evidence: "passthrough".to_string(),
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
            route_trace: RoutingTrace {
                source: crate::routing::RouteSource::Cli,
                selection_kind: SelectionKind::Fixed,
                match_evidence: MatchEvidence::Passthrough,
                harness: "cursor".to_string(),
                harness_order_position: None,
                candidates_tried: vec!["cursor".to_string()],
                assessments: Vec::new(),
                diagnostics: Vec::new(),
            },
        })
        .expect("routing should resolve");

        assert!(!resolution.effort_consumed);
        assert_eq!(
            resolution.cursor_effort_outcome,
            CursorEffortOutcome::NotRequested
        );
        assert_eq!(resolution.routing.model, "");
        assert_eq!(resolution.routing.harness_model, "");
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
