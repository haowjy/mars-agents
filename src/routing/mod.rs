use std::collections::HashSet;

use serde::Serialize;

use crate::models;
use crate::models::availability::AvailabilityStatus;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::OpenCodeProbeResult;

/// Confidence in a harness selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteConfidence {
    Explicit,
    Confirmed,
    Likely,
    Passthrough,
}

impl RouteConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Confirmed => "confirmed",
            Self::Likely => "likely",
            Self::Passthrough => "passthrough",
        }
    }
}

/// How the harness was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    Cli,
    Profile,
    Alias,
    ConfigOrder,
    ConfigDefault,
    Provider,
    HardcodedDefault,
}

impl RouteSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Profile => "profile",
            Self::Alias => "alias",
            Self::ConfigOrder => "config-order",
            Self::ConfigDefault => "config",
            Self::Provider => "provider",
            Self::HardcodedDefault => "default",
        }
    }
}

/// Assessment of one candidate harness.
#[derive(Debug, Clone)]
pub struct CandidateAssessment {
    pub harness: String,
    pub installed: bool,
    pub confidence: Option<RouteConfidence>,
    pub skip_reason: Option<&'static str>,
}

/// Full routing trace for diagnostics/provenance.
#[derive(Debug, Clone)]
pub struct RoutingTrace {
    pub source: RouteSource,
    pub confidence: RouteConfidence,
    pub harness: String,
    pub harness_order_position: Option<usize>,
    pub candidates_tried: Vec<String>,
    pub assessments: Vec<CandidateAssessment>,
    pub diagnostics: Vec<String>,
}

/// Input to the routing engine.
pub struct RoutingInput<'a> {
    pub model_id: &'a str,
    pub provider: Option<&'a str>,
    pub settings_harness_order: Option<&'a [String]>,
    pub config_default_harness: Option<&'a str>,
    pub installed_harnesses: &'a HashSet<String>,
    pub linked_harnesses: Option<&'a [String]>,
    pub opencode_probe_result: Option<&'a OpenCodeProbeResult>,
}

/// Evaluate all candidates and return a routing trace.
/// This is the ONLY candidate evaluator. Both `mars models` and `mars build` call this.
pub fn evaluate_candidates(input: &RoutingInput<'_>) -> RoutingTrace {
    evaluate_candidates_with_auth(input, models::harness::native_harness_authenticated)
}

pub fn evaluate_candidates_with_auth<F>(input: &RoutingInput<'_>, auth_check: F) -> RoutingTrace
where
    F: Fn(&str) -> bool,
{
    let mut diagnostics = Vec::new();
    let config_default_harness =
        normalize_config_default_harness(input.config_default_harness, &mut diagnostics);

    let linked_harnesses = input
        .linked_harnesses
        .map(|harnesses| harnesses.iter().map(String::as_str).collect::<HashSet<_>>());

    let mut harness_order_failure = None;

    let mut candidate_source = RouteSource::Provider;

    let candidates = if let Some(order) = input.settings_harness_order {
        let parsed_order = models::harness::parse_settings_harness_order(order);
        diagnostics.extend(parsed_order.warnings);

        if parsed_order.failure == Some(HarnessOrderFailure::Empty) {
            diagnostics.push(
                "settings.harness_order is empty; falling through to provider candidate order"
                    .to_string(),
            );
            let provider_for_order = input.provider.unwrap_or("unknown");
            filter_candidates_by_links(
                models::harness::harness_candidates_for_provider(provider_for_order),
                linked_harnesses.as_ref(),
            )
            .into_iter()
            .map(|harness| (harness, None))
            .collect::<Vec<_>>()
        } else {
            candidate_source = RouteSource::ConfigOrder;
            let mut candidate_pairs = parsed_order
                .valid_candidates
                .into_iter()
                .enumerate()
                .map(|(index, harness)| (harness, Some(index)))
                .collect::<Vec<_>>();

            filter_candidate_pairs_by_links(&mut candidate_pairs, linked_harnesses.as_ref());

            let valid_candidates = candidate_pairs
                .iter()
                .map(|(harness, _)| harness.clone())
                .collect::<Vec<_>>();

            if !valid_candidates.is_empty()
                && valid_candidates
                    .iter()
                    .all(|candidate| !input.installed_harnesses.contains(candidate))
            {
                harness_order_failure = Some(HarnessOrderFailure::NoneInstalled {
                    valid_candidates: valid_candidates.clone(),
                });
            }

            candidate_pairs
        }
    } else {
        let provider_for_order = input.provider.unwrap_or("unknown");
        filter_candidates_by_links(
            models::harness::harness_candidates_for_provider(provider_for_order),
            linked_harnesses.as_ref(),
        )
        .into_iter()
        .map(|harness| (harness, None))
        .collect::<Vec<_>>()
    };

    let mut candidates_tried = Vec::new();
    let mut assessments = Vec::new();

    for (harness, harness_order_position) in candidates {
        let assessment = candidate_route_confidence_with_auth(
            &harness,
            input.provider,
            input.model_id,
            input.installed_harnesses,
            input.opencode_probe_result,
            &auth_check,
        );

        candidates_tried.push(harness.clone());
        let confidence = assessment.confidence;
        assessments.push(assessment);

        if let Some(confidence) = confidence {
            return RoutingTrace {
                source: candidate_source,
                confidence,
                harness,
                harness_order_position,
                candidates_tried,
                assessments,
                diagnostics,
            };
        }
    }

    if input.settings_harness_order.is_some()
        && let Some(warning) = format_harness_order_fallback_warning(
            harness_order_failure.as_ref(),
            config_default_harness.is_some(),
        )
    {
        diagnostics.push(warning);
    }

    if let Some(harness) = config_default_harness {
        return RoutingTrace {
            source: RouteSource::ConfigDefault,
            confidence: RouteConfidence::Passthrough,
            harness,
            harness_order_position: None,
            candidates_tried,
            assessments,
            diagnostics,
        };
    }

    diagnostics.push(
        "harness not set by CLI/profile/alias/provider/config; defaulting to `claude`".into(),
    );

    RoutingTrace {
        source: RouteSource::HardcodedDefault,
        confidence: RouteConfidence::Passthrough,
        harness: "claude".to_string(),
        harness_order_position: None,
        candidates_tried,
        assessments,
        diagnostics,
    }
}

/// Normalize and validate config default_harness. Returns normalized name or None with warning.
pub fn normalize_config_default_harness(
    config_default_harness: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match config_default_harness {
        Some(value) => match models::harness::normalize_harness_name(value) {
            Some(valid) => Some(valid),
            None => {
                warnings.push(format!(
                    "settings.default_harness `{value}` is invalid; expected one of: {}",
                    models::harness::VALID_HARNESSES.join(", ")
                ));
                None
            }
        },
        None => None,
    }
}

fn filter_candidate_pairs_by_links(
    candidates: &mut Vec<(String, Option<usize>)>,
    linked_harnesses: Option<&HashSet<&str>>,
) {
    if let Some(linked_harnesses) = linked_harnesses {
        candidates.retain(|(harness, _)| linked_harnesses.contains(harness.as_str()));
    }
}

fn filter_candidates_by_links(
    candidates: Vec<String>,
    linked_harnesses: Option<&HashSet<&str>>,
) -> Vec<String> {
    let Some(linked_harnesses) = linked_harnesses else {
        return candidates;
    };

    candidates
        .into_iter()
        .filter(|harness| linked_harnesses.contains(harness.as_str()))
        .collect()
}

fn candidate_route_confidence_with_auth<F>(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    auth_check: &F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
{
    if !installed_harnesses.contains(harness) {
        return CandidateAssessment {
            harness: harness.to_string(),
            installed: false,
            confidence: None,
            skip_reason: Some("not_installed"),
        };
    }

    if is_native_match(provider, harness) {
        if auth_check(harness) {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                confidence: Some(RouteConfidence::Confirmed),
                skip_reason: None,
            };
        }

        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            confidence: None,
            skip_reason: Some("native_auth_unavailable"),
        };
    }

    if harness == "opencode" {
        if provider.is_none() || provider.is_some_and(|value| !is_known_provider(value)) {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                confidence: Some(RouteConfidence::Passthrough),
                skip_reason: None,
            };
        }

        if opencode_supports_provider_and_model(
            provider,
            model_id,
            installed_harnesses,
            opencode_probe_result,
        ) {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                confidence: Some(RouteConfidence::Likely),
                skip_reason: None,
            };
        }

        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            confidence: None,
            skip_reason: Some("opencode_unavailable"),
        };
    }

    if matches!(harness, "pi" | "cursor") {
        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            confidence: Some(RouteConfidence::Passthrough),
            skip_reason: None,
        };
    }

    CandidateAssessment {
        harness: harness.to_string(),
        installed: true,
        confidence: None,
        skip_reason: Some("unsupported_candidate"),
    }
}

fn is_known_provider(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "anthropic" | "openai" | "google" | "meta" | "mistral" | "deepseek" | "cohere"
    )
}

fn is_native_match(provider: Option<&str>, harness: &str) -> bool {
    matches!(
        (provider.map(str::to_ascii_lowercase).as_deref(), harness),
        (Some("anthropic"), "claude") | (Some("openai"), "codex")
    )
}

fn opencode_supports_provider_and_model(
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
) -> bool {
    let Some(provider) = provider else {
        return false;
    };

    matches!(
        crate::models::availability::classify_for_harness(
            "opencode",
            provider,
            model_id,
            installed_harnesses,
            opencode_probe_result,
        ),
        Some((AvailabilityStatus::Runnable, _, _))
    )
}

fn format_harness_order_fallback_warning(
    harness_order_failure: Option<&HarnessOrderFailure>,
    has_config_default_harness: bool,
) -> Option<String> {
    let mut warning = match harness_order_failure {
        Some(HarnessOrderFailure::Empty) => "settings.harness_order is empty".to_string(),
        Some(HarnessOrderFailure::NoneInstalled { valid_candidates }) => format!(
            "settings.harness_order is set but none of [{}] are installed",
            valid_candidates.join(", ")
        ),
        None => return None,
    };

    if has_config_default_harness {
        warning.push_str("; falling through to settings.default_harness");
    } else {
        warning
            .push_str("; settings.default_harness is unset, falling through to hardcoded `claude`");
    }

    Some(warning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn always_authed(_: &str) -> bool {
        true
    }

    fn never_authed(_: &str) -> bool {
        false
    }

    fn routing_input<'a>(
        model_id: &'a str,
        provider: Option<&'a str>,
        settings_harness_order: Option<&'a [String]>,
        config_default_harness: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        linked_harnesses: Option<&'a [String]>,
        opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    ) -> RoutingInput<'a> {
        RoutingInput {
            model_id,
            provider,
            settings_harness_order,
            config_default_harness,
            installed_harnesses,
            linked_harnesses,
            opencode_probe_result,
        }
    }

    #[test]
    fn native_match_with_auth_returns_confirmed() {
        let installed = installed(&["claude"]);
        let input = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.harness, "claude");
        assert_eq!(trace.confidence, RouteConfidence::Confirmed);
        assert_eq!(trace.candidates_tried, vec!["claude".to_string()]);
    }

    #[test]
    fn native_match_without_auth_falls_through() {
        let installed = installed(&["claude", "pi"]);
        let input = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
        assert_eq!(trace.candidates_tried, vec!["claude", "pi"]);
        assert_eq!(
            trace
                .assessments
                .first()
                .and_then(|assessment| assessment.skip_reason),
            Some("native_auth_unavailable")
        );
    }

    #[test]
    fn pi_or_cursor_installed_returns_passthrough() {
        let installed = installed(&["cursor"]);
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
    }

    #[test]
    fn opencode_positive_probe_returns_likely() {
        let installed = installed(&["opencode"]);
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("openai".to_string(), true)]),
            model_slugs: vec!["openai/gpt-5".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            Some(&probe),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "opencode");
        assert_eq!(trace.confidence, RouteConfidence::Likely);
    }

    #[test]
    fn opencode_negative_probe_falls_through() {
        let installed = installed(&["opencode", "cursor"]);
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("google".to_string(), true)]),
            model_slugs: Vec::new(),
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            Some(&probe),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "opencode")
                .and_then(|assessment| assessment.skip_reason),
            Some("opencode_unavailable")
        );
    }

    #[test]
    fn link_filtering_reduces_candidates() {
        let installed = installed(&["codex", "pi"]);
        let linked_harnesses = vec!["pi".to_string()];
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.candidates_tried, vec!["pi"]);
    }

    #[test]
    fn settings_harness_order_overrides_provider_order() {
        let installed = installed(&["codex", "pi"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::ConfigOrder);
        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.harness_order_position, Some(0));
    }

    #[test]
    fn empty_harness_order_falls_through_to_provider() {
        let installed = installed(&["codex"]);
        let order: Vec<String> = Vec::new();
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.harness, "codex");
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("settings.harness_order is empty"))
        );
    }

    #[test]
    fn uses_config_default_fallback() {
        let installed = installed(&[]);
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("Pi"),
            &installed,
            None,
            None,
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
    }

    #[test]
    fn uses_hardcoded_claude_fallback_with_warning() {
        let installed = installed(&[]);
        let input = routing_input("model", None, None, None, &installed, None, None);

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::HardcodedDefault);
        assert_eq!(trace.harness, "claude");
        assert!(trace.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains("harness not set by CLI/profile/alias/provider/config")
        }));
    }

    #[test]
    fn all_candidates_filtered_by_links_uses_config_or_hardcoded_fallback() {
        let installed = installed(&["codex"]);
        let linked_harnesses = vec!["claude".to_string()];

        let with_config_default = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("pi"),
            &installed,
            Some(&linked_harnesses),
            None,
        );
        let with_default_trace = evaluate_candidates_with_auth(&with_config_default, never_authed);
        assert_eq!(with_default_trace.candidates_tried, Vec::<String>::new());
        assert_eq!(with_default_trace.source, RouteSource::ConfigDefault);
        assert_eq!(with_default_trace.harness, "pi");

        let without_config_default = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            None,
        );
        let hardcoded_trace = evaluate_candidates_with_auth(&without_config_default, never_authed);
        assert_eq!(hardcoded_trace.source, RouteSource::HardcodedDefault);
        assert_eq!(hardcoded_trace.harness, "claude");
    }
}
