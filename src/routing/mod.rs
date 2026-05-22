use std::collections::HashSet;

use serde::Serialize;

use crate::models;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;

/// Confidence in a harness selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteConfidence {
    Confirmed,
    Constrained,
    Forced,
    Passthrough,
}

impl RouteConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Constrained => "constrained",
            Self::Forced => "forced",
            Self::Passthrough => "passthrough",
        }
    }
}

/// How the harness was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Serialize)]
pub struct CandidateAssessment {
    pub harness: String,
    pub installed: bool,
    pub candidate_slugs: Vec<String>,
    pub filtered_slugs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<RouteConfidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<&'static str>,
}

/// Full routing trace for diagnostics/provenance.
#[derive(Debug, Clone, Serialize)]
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
    pub provider_for_order: Option<&'a str>,
    pub provider_constraint: Option<&'a str>,
    pub settings_provider_order: Option<&'a [String]>,
    pub settings_harness_order: Option<&'a [String]>,
    pub config_default_harness: Option<&'a str>,
    pub installed_harnesses: &'a HashSet<String>,
    pub linked_harnesses: Option<&'a [String]>,
    pub opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub pi_probe_result: Option<&'a PiProbeResult>,
}

/// Evaluate all candidates and return a routing trace.
/// This is the ONLY candidate evaluator. Both `mars models` and `mars build` call this.
pub fn evaluate_candidates(input: &RoutingInput<'_>) -> RoutingTrace {
    evaluate_candidates_with_auth(input, models::harness::native_harness_authenticated)
}

/// Evaluate one fixed harness choice without fallback.
/// Used by fixed-selection precedence paths (CLI/profile/alias).
pub fn evaluate_fixed_harness(input: &RoutingInput<'_>, harness: &str) -> CandidateAssessment {
    evaluate_fixed_harness_with_auth(
        input,
        harness,
        models::harness::native_harness_authenticated,
    )
}

pub fn evaluate_fixed_harness_with_auth<F>(
    input: &RoutingInput<'_>,
    harness: &str,
    auth_check: F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
{
    candidate_route_confidence_with_auth(input, harness, input.settings_provider_order, &auth_check)
}

pub fn evaluate_candidates_with_auth<F>(input: &RoutingInput<'_>, auth_check: F) -> RoutingTrace
where
    F: Fn(&str) -> bool,
{
    let mut diagnostics = Vec::new();
    let parsed_provider_order =
        parse_settings_provider_order(input.settings_provider_order, &mut diagnostics);
    let config_default_harness =
        normalize_config_default_harness(input.config_default_harness, &mut diagnostics);
    let linked_harnesses = input
        .linked_harnesses
        .filter(|harnesses| !harnesses.is_empty());
    let linked_harnesses_set = linked_harnesses
        .map(|harnesses| harnesses.iter().map(String::as_str).collect::<HashSet<_>>());
    let has_link_constraints = linked_harnesses_set.is_some();
    let effective_config_default_harness = config_default_harness
        .as_ref()
        .filter(|harness| {
            linked_harnesses_set
                .as_ref()
                .is_none_or(|known| known.contains(harness.as_str()))
        })
        .cloned();
    if has_link_constraints
        && config_default_harness.is_some()
        && effective_config_default_harness.is_none()
    {
        diagnostics.push(
            "settings.default_harness is excluded by known linked harness constraints; ignoring fallback"
                .to_string(),
        );
    }

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
            let provider_for_order = input.provider_for_order.unwrap_or("unknown");
            filter_candidates_by_links(
                models::harness::harness_candidates_for_provider(provider_for_order),
                linked_harnesses_set.as_ref(),
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

            filter_candidate_pairs_by_links(&mut candidate_pairs, linked_harnesses_set.as_ref());

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
        let provider_for_order = input.provider_for_order.unwrap_or("unknown");
        filter_candidates_by_links(
            models::harness::harness_candidates_for_provider(provider_for_order),
            linked_harnesses_set.as_ref(),
        )
        .into_iter()
        .map(|harness| (harness, None))
        .collect::<Vec<_>>()
    };

    let mut candidates_tried = Vec::new();
    let mut assessments = Vec::new();

    for (harness, harness_order_position) in candidates {
        let assessment = candidate_route_confidence_with_auth(
            input,
            &harness,
            Some(parsed_provider_order.as_slice()),
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
            effective_config_default_harness.is_some(),
            has_link_constraints,
        )
    {
        diagnostics.push(warning);
    }

    if let Some(harness) = effective_config_default_harness {
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

    if let Some(known_links) = linked_harnesses {
        let harness = known_links
            .first()
            .expect("linked_harnesses is non-empty")
            .clone();
        diagnostics.push(format!(
            "known linked harness constraints left no eligible auto-routing candidates; selecting linked harness `{harness}` without unrelated fallback"
        ));
        candidates_tried.push(harness.clone());

        return RoutingTrace {
            source: candidate_source,
            confidence: RouteConfidence::Passthrough,
            harness,
            harness_order_position: None,
            candidates_tried,
            assessments,
            diagnostics,
        };
    }

    diagnostics
        .push("harness not set by CLI/profile/alias/provider/config; defaulting to `pi`".into());

    RoutingTrace {
        source: RouteSource::HardcodedDefault,
        confidence: RouteConfidence::Passthrough,
        harness: "pi".to_string(),
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
    input: &RoutingInput<'_>,
    harness: &str,
    provider_order: Option<&[String]>,
    auth_check: &F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
{
    if !input.installed_harnesses.contains(harness) {
        return CandidateAssessment {
            harness: harness.to_string(),
            installed: false,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            confidence: None,
            skip_reason: Some("not_installed"),
        };
    }

    if is_native_match(input.provider_for_order, harness) {
        if auth_check(harness) {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: Some(input.model_id.to_string()),
                confidence: Some(confidence_for_match(input.provider_constraint)),
                skip_reason: None,
            };
        }

        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            confidence: None,
            skip_reason: Some("native_auth_unavailable"),
        };
    }

    if harness == "opencode" {
        let Some(opencode_probe) = input.opencode_probe_result else {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                confidence: Some(RouteConfidence::Passthrough),
                skip_reason: None,
            };
        };
        if !opencode_probe.model_probe_success {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                confidence: Some(RouteConfidence::Passthrough),
                skip_reason: None,
            };
        }

        let selection = select_probe_slug(
            input.model_id,
            input.provider_constraint,
            provider_order,
            opencode_probe.model_slugs.iter().map(String::as_str),
        );

        if let Some(chosen_slug) = selection.chosen_slug.clone() {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: selection.candidate_slugs,
                filtered_slugs: selection.filtered_slugs,
                chosen_model: model_from_slug(&chosen_slug),
                chosen_slug: Some(chosen_slug),
                confidence: Some(confidence_for_match(input.provider_constraint)),
                skip_reason: None,
            };
        }

        if !selection.candidate_slugs.is_empty() {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: selection.candidate_slugs,
                filtered_slugs: selection.filtered_slugs,
                chosen_slug: None,
                chosen_model: None,
                confidence: None,
                skip_reason: Some("provider_constraint_unsatisfied"),
            };
        }

        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: selection.candidate_slugs,
            filtered_slugs: selection.filtered_slugs,
            chosen_slug: None,
            chosen_model: None,
            confidence: None,
            skip_reason: Some("no_model_match"),
        };
    }

    if harness == "pi" {
        if let Some(pi_probe) = input.pi_probe_result {
            if pi_probe.compatible {
                let selection = select_probe_slug(
                    input.model_id,
                    input.provider_constraint,
                    provider_order,
                    pi_probe.model_slugs.iter().map(String::as_str),
                );

                if let Some(chosen_slug) = selection.chosen_slug.clone() {
                    return CandidateAssessment {
                        harness: harness.to_string(),
                        installed: true,
                        candidate_slugs: selection.candidate_slugs,
                        filtered_slugs: selection.filtered_slugs,
                        chosen_model: model_from_slug(&chosen_slug),
                        chosen_slug: Some(chosen_slug),
                        confidence: Some(confidence_for_match(input.provider_constraint)),
                        skip_reason: None,
                    };
                }

                if !selection.candidate_slugs.is_empty() {
                    return CandidateAssessment {
                        harness: harness.to_string(),
                        installed: true,
                        candidate_slugs: selection.candidate_slugs,
                        filtered_slugs: selection.filtered_slugs,
                        chosen_slug: None,
                        chosen_model: None,
                        confidence: None,
                        skip_reason: Some("provider_constraint_unsatisfied"),
                    };
                }

                return CandidateAssessment {
                    harness: harness.to_string(),
                    installed: true,
                    candidate_slugs: selection.candidate_slugs,
                    filtered_slugs: selection.filtered_slugs,
                    chosen_slug: None,
                    chosen_model: None,
                    confidence: None,
                    skip_reason: Some("no_model_match"),
                };
            }
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                confidence: None,
                skip_reason: Some("pi_incompatible"),
            };
        }

        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            confidence: Some(RouteConfidence::Passthrough),
            skip_reason: None,
        };
    }

    if harness == "cursor" {
        return CandidateAssessment {
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            confidence: Some(RouteConfidence::Passthrough),
            skip_reason: None,
        };
    }

    CandidateAssessment {
        harness: harness.to_string(),
        installed: true,
        candidate_slugs: Vec::new(),
        filtered_slugs: Vec::new(),
        chosen_slug: None,
        chosen_model: None,
        confidence: None,
        skip_reason: Some("unsupported_candidate"),
    }
}

fn is_native_match(provider: Option<&str>, harness: &str) -> bool {
    matches!(
        (provider.map(str::to_ascii_lowercase).as_deref(), harness),
        (Some("anthropic"), "claude") | (Some("openai"), "codex")
    )
}

fn confidence_for_match(provider_constraint: Option<&str>) -> RouteConfidence {
    if provider_constraint.is_some() {
        RouteConfidence::Constrained
    } else {
        RouteConfidence::Confirmed
    }
}

fn parse_settings_provider_order(
    provider_order: Option<&[String]>,
    diagnostics: &mut Vec<String>,
) -> Vec<String> {
    let Some(provider_order) = provider_order else {
        return Vec::new();
    };

    provider_order
        .iter()
        .filter_map(|provider| {
            let normalized = provider.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                return None;
            }
            if !is_known_provider_or_variant(&normalized) {
                diagnostics.push(format!(
                    "settings.provider_order contains unknown provider `{provider}`; keeping it for forward-compat routing preferences"
                ));
            }
            Some(normalized)
        })
        .collect()
}

fn is_known_provider_or_variant(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic"
            | "openai"
            | "google"
            | "meta"
            | "mistral"
            | "deepseek"
            | "cohere"
            | "openrouter"
            | "openai-codex"
            | "anthropic-claude"
    )
}

fn provider_key_for_order(provider: &str) -> String {
    let normalized = provider.trim().to_ascii_lowercase();
    if let Some(base) = normalized.strip_suffix("-codex")
        && base == "openai"
    {
        return base.to_string();
    }
    if let Some(base) = normalized.strip_suffix("-claude")
        && base == "anthropic"
    {
        return base.to_string();
    }
    normalized
}

fn parse_slug(slug: &str) -> Option<(&str, &str)> {
    let (provider, model_id) = slug.split_once('/')?;
    (!provider.is_empty() && !model_id.is_empty()).then_some((provider, model_id))
}

fn model_from_slug(slug: &str) -> Option<String> {
    parse_slug(slug).map(|(_, model)| model.to_string())
}

struct SlugSelection {
    candidate_slugs: Vec<String>,
    filtered_slugs: Vec<String>,
    chosen_slug: Option<String>,
}

fn select_probe_slug<'a>(
    model_id: &str,
    provider_constraint: Option<&str>,
    provider_order: Option<&[String]>,
    slugs: impl IntoIterator<Item = &'a str>,
) -> SlugSelection {
    let mut model_matches = Vec::new();
    for (index, slug) in slugs.into_iter().enumerate() {
        let Some((provider, slug_model_id)) = parse_slug(slug) else {
            continue;
        };
        if crate::models::availability::model_id_matches(model_id, slug_model_id) {
            model_matches.push((index, provider.to_ascii_lowercase(), slug.to_string()));
        }
    }
    let candidate_slugs = model_matches
        .iter()
        .map(|(_, _, slug)| slug.clone())
        .collect::<Vec<_>>();

    let mut constrained_matches = model_matches;
    if let Some(constraint) = provider_constraint {
        let normalized_constraint = constraint.trim().to_ascii_lowercase();
        constrained_matches.retain(|(_, provider, _)| provider == &normalized_constraint);
    }
    let filtered_slugs = constrained_matches
        .iter()
        .map(|(_, _, slug)| slug.clone())
        .collect::<Vec<_>>();

    let chosen_slug = if constrained_matches.is_empty() {
        None
    } else if let Some(provider_order) = provider_order {
        if provider_order.is_empty() {
            constrained_matches
                .sort_by(|(left_index, _, _), (right_index, _, _)| left_index.cmp(right_index));
        } else {
            constrained_matches.sort_by(
                |(left_index, left_provider, _), (right_index, right_provider, _)| {
                    provider_order_rank(left_provider, provider_order)
                        .cmp(&provider_order_rank(right_provider, provider_order))
                        .then_with(|| left_index.cmp(right_index))
                },
            );
        }
        constrained_matches.first().map(|(_, _, slug)| slug.clone())
    } else {
        constrained_matches
            .iter()
            .min_by_key(|(index, _, _)| *index)
            .map(|(_, _, slug)| slug.clone())
    };

    SlugSelection {
        candidate_slugs,
        filtered_slugs,
        chosen_slug,
    }
}

fn provider_order_rank(provider: &str, provider_order: &[String]) -> usize {
    let key = provider_key_for_order(provider);
    provider_order
        .iter()
        .position(|configured| provider_key_for_order(configured) == key)
        .unwrap_or(usize::MAX)
}

fn format_harness_order_fallback_warning(
    harness_order_failure: Option<&HarnessOrderFailure>,
    has_config_default_harness: bool,
    has_link_constraints: bool,
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
    } else if has_link_constraints {
        warning.push_str("; linked harness constraints prevent unrelated fallback");
    } else {
        warning.push_str("; settings.default_harness is unset, falling through to hardcoded `pi`");
    }

    Some(warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn always_authed(_: &str) -> bool {
        true
    }

    fn never_authed(_: &str) -> bool {
        false
    }

    type ProbeInputs<'a> = (Option<&'a OpenCodeProbeResult>, Option<&'a PiProbeResult>);

    fn routing_input<'a>(
        model_id: &'a str,
        provider_for_order: Option<&'a str>,
        settings_harness_order: Option<&'a [String]>,
        config_default_harness: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        linked_harnesses: Option<&'a [String]>,
        probe_inputs: ProbeInputs<'a>,
    ) -> RoutingInput<'a> {
        let (opencode_probe_result, pi_probe_result) = probe_inputs;
        RoutingInput {
            model_id,
            provider_for_order,
            provider_constraint: None,
            settings_provider_order: None,
            settings_harness_order,
            config_default_harness,
            installed_harnesses,
            linked_harnesses,
            opencode_probe_result,
            pi_probe_result,
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
            (None, None),
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
            (None, None),
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
            (None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
    }

    #[test]
    fn compatible_pi_probe_returns_confirmed() {
        let installed = installed(&["pi"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["google/gemini-2.5-pro".to_string()]),
            ..PiProbeResult::default()
        };
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            (None, Some(&pi_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.confidence, RouteConfidence::Confirmed);
    }

    #[test]
    fn strict_provider_constraint_rejects_variant_provider_name() {
        let installed = installed(&["pi", "opencode"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai-codex/gpt-5.4-mini".to_string()]),
            ..PiProbeResult::default()
        };
        let opencode_probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4-mini".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = RoutingInput {
            model_id: "gpt-5.4-mini",
            provider_for_order: Some("openai"),
            provider_constraint: Some("openai"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: Some(&opencode_probe),
            pi_probe_result: Some(&pi_probe),
        };

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "opencode");
        assert_eq!(trace.confidence, RouteConfidence::Constrained);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "opencode")
                .and_then(|assessment| assessment.chosen_slug.as_deref()),
            Some("openai/gpt-5.4-mini")
        );
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.skip_reason),
            Some("provider_constraint_unsatisfied")
        );
    }

    #[test]
    fn bare_direct_model_prefers_unknown_provider_ladder_and_pi_slug() {
        let installed = installed(&["codex", "pi", "opencode"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai-codex/gpt-5.4".to_string()]),
            ..PiProbeResult::default()
        };
        let input = RoutingInput {
            model_id: "gpt-5.4",
            provider_for_order: None,
            provider_constraint: None,
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: Some(&pi_probe),
        };

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.confidence, RouteConfidence::Confirmed);
        assert_eq!(trace.candidates_tried, vec!["pi".to_string()]);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.chosen_slug.as_deref()),
            Some("openai-codex/gpt-5.4")
        );
    }

    #[test]
    fn provider_order_ranking_is_lenient_for_known_variants() {
        let provider_order = vec!["openai".to_string(), "anthropic".to_string()];
        assert_eq!(provider_order_rank("openai-codex", &provider_order), 0);
        assert_eq!(provider_order_rank("anthropic-claude", &provider_order), 1);
        assert_eq!(
            provider_order_rank("openrouter", &provider_order),
            usize::MAX
        );
    }

    #[test]
    fn incompatible_pi_probe_skips_to_next_candidate() {
        let installed = installed(&["pi", "cursor"]);
        let pi_probe = PiProbeResult {
            compatible: false,
            ..PiProbeResult::default()
        };
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            (None, Some(&pi_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.skip_reason),
            Some("pi_incompatible")
        );
    }

    #[test]
    fn opencode_positive_probe_returns_likely() {
        let installed = installed(&["opencode"]);
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5".to_string()],
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
            (Some(&probe), None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "opencode");
        assert_eq!(trace.confidence, RouteConfidence::Confirmed);
    }

    #[test]
    fn opencode_negative_probe_falls_through() {
        let installed = installed(&["opencode", "cursor"]);
        let probe = OpenCodeProbeResult {
            model_slugs: Vec::new(),
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
            (Some(&probe), None),
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
            Some("no_model_match")
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
            (None, None),
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
            (None, None),
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
            (None, None),
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
            (None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.confidence, RouteConfidence::Passthrough);
    }

    #[test]
    fn uses_hardcoded_pi_fallback_with_warning() {
        let installed = installed(&[]);
        let input = routing_input("model", None, None, None, &installed, None, (None, None));

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::HardcodedDefault);
        assert_eq!(trace.harness, "pi");
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.contains("defaulting to `pi`") })
        );
    }

    #[test]
    fn linked_constraints_apply_to_default_and_hardcoded_fallbacks() {
        let installed = installed(&["codex"]);
        let linked_harnesses = vec!["claude".to_string()];

        let with_config_default = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("pi"),
            &installed,
            Some(&linked_harnesses),
            (None, None),
        );
        let with_default_trace = evaluate_candidates_with_auth(&with_config_default, never_authed);
        assert_eq!(with_default_trace.source, RouteSource::Provider);
        assert_eq!(with_default_trace.harness, "claude");
        assert_eq!(with_default_trace.candidates_tried, vec!["claude"]);
        assert!(with_default_trace.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains(
                "settings.default_harness is excluded by known linked harness constraints",
            )
        }));

        let without_config_default = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            (None, None),
        );
        let hardcoded_trace = evaluate_candidates_with_auth(&without_config_default, never_authed);
        assert_eq!(hardcoded_trace.source, RouteSource::Provider);
        assert_eq!(hardcoded_trace.harness, "claude");
        assert!(
            hardcoded_trace
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.contains("without unrelated fallback") })
        );
    }

    #[test]
    fn linked_default_harness_is_allowed_when_linked() {
        let installed = installed(&[]);
        let linked_harnesses = vec!["pi".to_string()];
        let trace = evaluate_candidates_with_auth(
            &routing_input(
                "gpt-5",
                Some("openai"),
                None,
                Some("pi"),
                &installed,
                Some(&linked_harnesses),
                (None, None),
            ),
            never_authed,
        );

        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.harness, "pi");
    }

    #[test]
    fn fixed_harness_evaluation_has_no_fallback() {
        let installed = installed(&[]);
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("pi"),
            &installed,
            None,
            (None, None),
        );
        let assessment = evaluate_fixed_harness_with_auth(&input, "codex", never_authed);

        assert_eq!(assessment.harness, "codex");
        assert!(!assessment.installed);
        assert_eq!(assessment.confidence, None);
        assert_eq!(assessment.skip_reason, Some("not_installed"));
    }
}
