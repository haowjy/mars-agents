// qa-validated: mars-capability-cache-resolver

use std::collections::HashSet;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models;
use crate::models::ModelAlias;
use crate::models::availability::AvailabilityStatus;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::OpenCodeProbeResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteConfidence {
    Explicit,
    Confirmed,
    Likely,
    Passthrough,
}

impl RouteConfidence {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Confirmed => "confirmed",
            Self::Likely => "likely",
            Self::Passthrough => "passthrough",
        }
    }
}

pub(super) struct HarnessResolution {
    pub(super) harness: String,
    pub(super) source: &'static str,
    pub(super) harness_order_position: Option<usize>,
    pub(super) route_confidence: RouteConfidence,
    pub(super) candidates_tried: Vec<String>,
    pub(super) is_experimental: bool,
    pub(super) resolved_harness: HarnessKind,
    pub(super) warnings: Vec<String>,
}

struct CandidateHarnessResolution {
    harness: String,
    source: &'static str,
    harness_order_position: Option<usize>,
    route_confidence: RouteConfidence,
    candidates_tried: Vec<String>,
    warnings: Vec<String>,
}

pub(super) struct HarnessEvidence<'a> {
    pub(super) model_id: &'a str,
    pub(super) provider: Option<&'a str>,
    pub(super) config_default_harness: Option<&'a str>,
    pub(super) harness_order: Option<&'a [String]>,
    pub(super) installed_harnesses: &'a HashSet<String>,
    pub(super) linked_harnesses: Option<&'a [String]>,
    pub(super) opencode_probe_result: Option<&'a OpenCodeProbeResult>,
}

pub(super) fn resolve_harness(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    evidence: HarnessEvidence<'_>,
) -> Result<HarnessResolution, MarsError> {
    let mut warnings = Vec::new();

    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let alias_harness = alias.and_then(|entry| entry.harness.as_deref());
    let normalized_config_default_harness =
        normalize_config_default_harness(evidence.config_default_harness, &mut warnings);

    let model_from_cli = input.model_override.is_some();
    let mut selected_harness_order_position = None;
    let (harness, harness_source, route_confidence, candidates_tried) =
        if let Some(harness) = input.harness_override {
            (
                harness.to_string(),
                "cli",
                RouteConfidence::Explicit,
                vec![harness.to_string()],
            )
        } else if model_from_cli {
            if let Some(harness) = alias_harness {
                (
                    harness.to_string(),
                    "alias",
                    RouteConfidence::Passthrough,
                    Vec::new(),
                )
            } else {
                let resolved = resolve_harness_candidate_or_fallback(
                    evidence.model_id,
                    evidence.provider,
                    evidence.harness_order,
                    evidence.installed_harnesses,
                    evidence.linked_harnesses,
                    normalized_config_default_harness.clone(),
                    evidence.opencode_probe_result,
                );
                selected_harness_order_position = resolved.harness_order_position;
                warnings.extend(resolved.warnings);
                (
                    resolved.harness,
                    resolved.source,
                    resolved.route_confidence,
                    resolved.candidates_tried,
                )
            }
        } else if let Some(harness) = profile_harness {
            (
                harness.to_string(),
                "profile",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else if let Some(harness) = alias_harness {
            (
                harness.to_string(),
                "alias",
                RouteConfidence::Passthrough,
                Vec::new(),
            )
        } else {
            let resolved = resolve_harness_candidate_or_fallback(
                evidence.model_id,
                evidence.provider,
                evidence.harness_order,
                evidence.installed_harnesses,
                evidence.linked_harnesses,
                normalized_config_default_harness,
                evidence.opencode_probe_result,
            );
            selected_harness_order_position = resolved.harness_order_position;
            warnings.extend(resolved.warnings);
            (
                resolved.harness,
                resolved.source,
                resolved.route_confidence,
                resolved.candidates_tried,
            )
        };

    let resolved_harness = HarnessKind::from_str(&harness).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "resolved harness `{harness}` is invalid; expected one of: claude, codex, opencode, cursor, pi"
            ),
        })
    })?;

    Ok(HarnessResolution {
        is_experimental: harness == "cursor",
        resolved_harness,
        harness,
        source: harness_source,
        harness_order_position: selected_harness_order_position,
        route_confidence,
        candidates_tried,
        warnings,
    })
}

fn normalize_config_default_harness(
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

fn resolve_harness_candidate_or_fallback(
    model_id: &str,
    provider: Option<&str>,
    settings_harness_order: Option<&[String]>,
    installed_harnesses: &HashSet<String>,
    linked_harnesses: Option<&[String]>,
    config_default_harness: Option<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
) -> CandidateHarnessResolution {
    let mut warnings = Vec::new();
    let mut candidates_tried = Vec::new();
    let mut harness_order_failure = None;

    let linked_harnesses = linked_harnesses
        .map(|harnesses| harnesses.iter().map(String::as_str).collect::<HashSet<_>>());

    let candidates = if let Some(order) = settings_harness_order {
        let parsed_order = models::harness::parse_settings_harness_order(order);
        warnings.extend(parsed_order.warnings);
        harness_order_failure = parsed_order.failure;
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
        if harness_order_failure.is_none()
            && !valid_candidates.is_empty()
            && valid_candidates
                .iter()
                .all(|candidate| !installed_harnesses.contains(candidate))
        {
            harness_order_failure = Some(HarnessOrderFailure::NoneInstalled {
                valid_candidates: valid_candidates.clone(),
            });
        }
        candidate_pairs
    } else {
        let provider_for_order = provider.unwrap_or("unknown");
        let candidates = models::harness::harness_candidates_for_provider(provider_for_order);
        filter_candidates_by_links(candidates, linked_harnesses.as_ref())
            .into_iter()
            .map(|harness| (harness, None))
            .collect::<Vec<_>>()
    };

    for (harness, harness_order_position) in candidates {
        candidates_tried.push(harness.clone());
        if let Some(route_confidence) = candidate_route_confidence(
            &harness,
            provider,
            model_id,
            installed_harnesses,
            opencode_probe_result,
        ) {
            let source = if settings_harness_order.is_some() {
                "config-order"
            } else {
                "provider"
            };
            return CandidateHarnessResolution {
                harness,
                source,
                harness_order_position,
                route_confidence,
                candidates_tried,
                warnings,
            };
        }
    }

    if settings_harness_order.is_some()
        && let Some(warning) = format_harness_order_fallback_warning(
            harness_order_failure.as_ref(),
            config_default_harness.is_some(),
        )
    {
        warnings.push(warning);
    }

    if let Some(harness) = config_default_harness {
        return CandidateHarnessResolution {
            harness,
            source: "config",
            harness_order_position: None,
            route_confidence: RouteConfidence::Passthrough,
            candidates_tried,
            warnings,
        };
    }

    warnings.push(
        "harness not set by CLI/profile/alias/provider/config; defaulting to `claude`".to_string(),
    );
    CandidateHarnessResolution {
        harness: "claude".to_string(),
        source: "default",
        harness_order_position: None,
        route_confidence: RouteConfidence::Passthrough,
        candidates_tried,
        warnings,
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

fn candidate_route_confidence(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
) -> Option<RouteConfidence> {
    candidate_route_confidence_with_auth(
        harness,
        provider,
        model_id,
        installed_harnesses,
        opencode_probe_result,
        native_harness_authenticated,
    )
}

/// Core routing logic, parameterized on auth checker for testability.
fn candidate_route_confidence_with_auth(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    auth_check: fn(&str) -> bool,
) -> Option<RouteConfidence> {
    if !installed_harnesses.contains(harness) {
        return None;
    }

    if is_native_match(provider, harness) && auth_check(harness) {
        return Some(RouteConfidence::Confirmed);
    }

    if harness == "opencode" {
        if provider.is_none() || provider.is_some_and(|value| !is_known_provider(value)) {
            return Some(RouteConfidence::Passthrough);
        }
        if opencode_supports_provider_and_model(
            provider,
            model_id,
            installed_harnesses,
            opencode_probe_result,
        ) {
            return Some(RouteConfidence::Likely);
        }
    }

    if matches!(harness, "pi" | "cursor") {
        return Some(RouteConfidence::Passthrough);
    }

    None
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

fn native_harness_authenticated(harness: &str) -> bool {
    models::harness::native_harness_authenticated(harness)
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

pub(super) fn harness_kind_to_str(harness: &HarnessKind) -> &'static str {
    match harness {
        HarnessKind::Claude => "claude",
        HarnessKind::Codex => "codex",
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Cursor => "cursor",
        HarnessKind::Pi => "pi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn always_authed(_: &str) -> bool {
        true
    }

    fn never_authed(_: &str) -> bool {
        false
    }

    // --- candidate_route_confidence_with_auth: key boundary tests ---
    // These test the auth-injection seam which cannot be exercised through
    // integration tests without forking real harness binaries.

    #[test]
    fn route_confidence_native_match_authed_returns_confirmed() {
        let result = candidate_route_confidence_with_auth(
            "claude",
            Some("anthropic"),
            "claude-opus-4-7",
            &installed(&["claude"]),
            None,
            always_authed,
        );
        assert_eq!(result, Some(RouteConfidence::Confirmed));
    }

    #[test]
    fn route_confidence_native_match_not_authed_falls_through() {
        // Auth gate: native match without auth must NOT return Confirmed.
        // claude is not pi/cursor/opencode so falls to None entirely.
        let result = candidate_route_confidence_with_auth(
            "claude",
            Some("anthropic"),
            "claude-opus-4-7",
            &installed(&["claude"]),
            None,
            never_authed,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn route_confidence_pi_installed_returns_passthrough() {
        let result = candidate_route_confidence_with_auth(
            "pi",
            Some("openai"),
            "gpt-5",
            &installed(&["pi"]),
            None,
            never_authed,
        );
        assert_eq!(result, Some(RouteConfidence::Passthrough));
    }

    #[test]
    fn route_confidence_opencode_positive_probe_returns_likely() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("openai".to_string(), true)]),
            model_slugs: vec!["openai/gpt-5".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };
        let result = candidate_route_confidence_with_auth(
            "opencode",
            Some("openai"),
            "gpt-5",
            &installed(&["opencode"]),
            Some(&probe),
            never_authed,
        );
        assert_eq!(result, Some(RouteConfidence::Likely));
    }

    #[test]
    fn route_confidence_opencode_negative_probe_returns_none() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("google".to_string(), true)]),
            model_slugs: vec![],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };
        // openai provider requested but probe says only google is available
        let result = candidate_route_confidence_with_auth(
            "opencode",
            Some("openai"),
            "gpt-5",
            &installed(&["opencode"]),
            Some(&probe),
            never_authed,
        );
        assert_eq!(result, None);
    }
}
