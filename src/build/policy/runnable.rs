use crate::build::bundle::Routing;
use crate::harness::registry::{self, HarnessId};
use crate::models::availability::{RunnableConfidence, RunnablePathSource};
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::cursor::{CursorEffortResolutionError, resolve_cursor_effort_slug};
use crate::routing::{MatchEvidence, report::RouteDecisionReport};

pub(super) struct RoutingInput<'a> {
    pub(super) model: String,
    pub(super) model_token: String,
    pub(super) harness: String,
    pub(super) selection_kind: String,
    pub(super) match_evidence: String,
    pub(super) provider_constraint: Option<&'a str>,
    pub(super) provider_for_order: Option<&'a str>,
    pub(super) effort: Option<String>,
    pub(super) cursor_probe_result: Option<&'a CursorProbeResult>,
    pub(super) route_report: RouteDecisionReport,
}

pub(super) struct RoutingResolution {
    pub(super) routing: Routing,
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
        cursor_probe_result,
        route_report,
    } = input;

    let selected_assessment = route_report
        .selected_attempt()
        .into_iter()
        .flat_map(|attempt| &attempt.assessments)
        .find(|assessment| assessment.harness == harness);
    let harness_id = registry::parse(&harness).expect("selected harness is registered");
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
        route_trace: route_report,
    };
    let mut effort_consumed = false;
    let mut cursor_effort_outcome = CursorEffortOutcome::NotRequested;

    if harness_id == HarnessId::Cursor
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

    RoutingResolution {
        routing,
        effort_consumed,
        cursor_effort_outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{RoutingTrace, SelectionKind};

    fn report(trace: RoutingTrace) -> RouteDecisionReport {
        let mut report = RouteDecisionReport::new(
            &crate::config::targets::HarnessScope::Unrestricted,
            &Default::default(),
            &[],
        );
        report.push("test", "test", "cli", &trace);
        report.select(0);
        report
    }

    fn trace_with_assessment(evidence: MatchEvidence) -> RoutingTrace {
        RoutingTrace {
            source: crate::routing::RouteSource::Provider,
            selection_kind: SelectionKind::Auto,
            selected_by_preference: false,
            match_evidence: evidence,
            harness: "opencode".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["opencode".to_string()],
            assessments: vec![crate::routing::CandidateAssessment {
                auth: None,
                harness: "opencode".to_string(),
                installed: true,
                candidate_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                filtered_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                chosen_slug: (evidence != MatchEvidence::Passthrough)
                    .then(|| "openai/gpt-5.4-mini".to_string()),
                chosen_model: (evidence != MatchEvidence::Passthrough)
                    .then(|| "gpt-5.4-mini".to_string()),
                match_evidence: Some(evidence),
                skip_reason: None,
            }],
            diagnostics: Vec::new(),
            exhaustion_reason: None,
        }
    }

    #[test]
    fn opencode_uses_selected_slug_from_route_report() {
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gptmini".to_string(),
            harness: "opencode".to_string(),
            selection_kind: "auto".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: Some("openai"),
            effort: None,
            cursor_probe_result: None,
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: None,
            cursor_probe_result: None,
            route_report: report(trace_with_assessment(MatchEvidence::Passthrough)),
        });

        assert_eq!(resolution.routing.harness_model, "gpt-5.4-mini".to_string());
    }

    #[test]
    fn pi_uses_selected_slug_for_bare_model() {
        let trace = RoutingTrace {
            source: crate::routing::RouteSource::Cli,
            selection_kind: SelectionKind::Fixed,
            selected_by_preference: false,
            match_evidence: MatchEvidence::Confirmed,
            harness: "pi".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["pi".to_string()],
            assessments: vec![crate::routing::CandidateAssessment {
                auth: None,
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
            exhaustion_reason: None,
        };
        let resolution = resolve_routing(RoutingInput {
            model: "gpt-5.4-mini".to_string(),
            model_token: "gpt-5.4-mini".to_string(),
            harness: "pi".to_string(),
            selection_kind: "fixed".to_string(),
            match_evidence: "confirmed".to_string(),
            provider_constraint: None,
            provider_for_order: Some("openai"),
            effort: None,
            cursor_probe_result: None,
            route_report: report(trace),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("medium".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec![
                    "gpt-5.5".to_string(),
                    "gpt-5.5-high".to_string(),
                    "gpt-5.5-low".to_string(),
                ],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["composer-2.5".to_string(), "composer-2.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec![
                    "composer-2.5".to_string(),
                    "composer-2.5-high".to_string(),
                    "composer-2.5-low".to_string(),
                ],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["gpt-5.5".to_string(), "gpt-5.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: None,
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: Vec::new(),
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: Vec::new(),
                model_probe_success: false,
                error: Some("model probe failed: timeout".to_string()),
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["claude-opus-4-7-high".to_string()],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(trace_with_assessment(MatchEvidence::Confirmed)),
        });

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
            effort: Some("high".to_string()),
            cursor_probe_result: Some(&crate::models::probes::CursorProbeResult {
                slugs: vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()],
                model_probe_success: true,
                error: None,
            }),
            route_report: report(RoutingTrace {
                source: crate::routing::RouteSource::Cli,
                selection_kind: SelectionKind::Fixed,
                selected_by_preference: false,
                match_evidence: MatchEvidence::Passthrough,
                harness: "cursor".to_string(),
                harness_order_position: None,
                candidates_tried: vec!["cursor".to_string()],
                assessments: Vec::new(),
                diagnostics: Vec::new(),
                exhaustion_reason: None,
            }),
        });

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
            effort: None,
            cursor_probe_result: None,
            route_report: report(RoutingTrace {
                source: crate::routing::RouteSource::Provider,
                selection_kind: SelectionKind::Auto,
                selected_by_preference: false,
                match_evidence: MatchEvidence::Passthrough,
                harness: "claude".to_string(),
                harness_order_position: None,
                candidates_tried: vec!["claude".to_string()],
                assessments: Vec::new(),
                diagnostics: Vec::new(),
                exhaustion_reason: None,
            }),
        });

        assert_eq!(resolution.routing.model, "");
        assert_eq!(resolution.routing.model_token, "");
        assert_eq!(resolution.routing.harness_model, "");
        assert_eq!(
            resolution.routing.harness_model_source,
            "passthrough".to_string()
        );
    }
}
