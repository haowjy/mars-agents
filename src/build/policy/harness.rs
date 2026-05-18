use std::collections::HashSet;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models;
use crate::models::ModelAlias;
use crate::models::availability::AvailabilityStatus;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::opencode_cache;

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

pub(super) fn resolve_harness(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
    model_id: &str,
    provider: Option<&str>,
    config_default_harness: Option<&str>,
    harness_order: Option<&[String]>,
) -> Result<HarnessResolution, MarsError> {
    let mut warnings = Vec::new();

    let profile_harness = input.profile.harness.as_ref().map(harness_kind_to_str);
    let alias_harness = alias.and_then(|entry| entry.harness.as_deref());
    let installed_harnesses = models::harness::detect_installed_harnesses();
    let normalized_config_default_harness =
        normalize_config_default_harness(config_default_harness, &mut warnings);

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
                    model_id,
                    provider,
                    harness_order,
                    &installed_harnesses,
                    normalized_config_default_harness.clone(),
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
                model_id,
                provider,
                harness_order,
                &installed_harnesses,
                normalized_config_default_harness,
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
        Some(value) => match normalize_harness_name(value) {
            Some(valid) => Some(valid.to_string()),
            None => {
                warnings.push(format!(
                    "settings.default_harness `{value}` is invalid; expected one of: claude, codex, opencode, cursor, pi"
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
    config_default_harness: Option<String>,
) -> CandidateHarnessResolution {
    let mut candidates_tried = Vec::new();
    if settings_harness_order.is_none() {
        let provider_resolution =
            resolve_provider_candidates(provider, model_id, installed_harnesses);
        if let Some(resolved) = provider_resolution.selected {
            return CandidateHarnessResolution {
                harness: resolved.harness,
                source: "provider",
                harness_order_position: None,
                route_confidence: resolved.route_confidence,
                candidates_tried: provider_resolution.candidates_tried,
                warnings: Vec::new(),
            };
        }
        candidates_tried = provider_resolution.candidates_tried;
    }

    let mut candidate = models::harness::resolve_harness_from_candidates(
        provider,
        settings_harness_order,
        installed_harnesses,
    );
    if let Some(harness) = candidate.harness {
        let route_confidence = route_confidence_for_selected_harness(
            &harness,
            provider,
            model_id,
            installed_harnesses,
        );
        let candidates_tried = if settings_harness_order.is_some() {
            vec![harness.clone()]
        } else {
            candidates_tried
        };
        return CandidateHarnessResolution {
            harness,
            source: candidate.source.unwrap_or("provider"),
            harness_order_position: candidate.harness_order_position,
            route_confidence,
            candidates_tried,
            warnings: candidate.warnings,
        };
    }

    if settings_harness_order.is_some()
        && let Some(warning) = format_harness_order_fallback_warning(
            candidate.harness_order_failure.as_ref(),
            config_default_harness.is_some(),
        )
    {
        candidate.warnings.push(warning);
    }

    if let Some(harness) = config_default_harness {
        return CandidateHarnessResolution {
            harness,
            source: "config",
            harness_order_position: None,
            route_confidence: RouteConfidence::Passthrough,
            candidates_tried,
            warnings: candidate.warnings,
        };
    }

    let mut warnings = candidate.warnings;
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

struct ProviderCandidateResult {
    selected: Option<SelectedProviderCandidate>,
    candidates_tried: Vec<String>,
}

struct SelectedProviderCandidate {
    harness: String,
    route_confidence: RouteConfidence,
}

fn resolve_provider_candidates(
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
) -> ProviderCandidateResult {
    let provider_for_order = provider.unwrap_or("unknown");
    let candidates = models::harness::harness_candidates_for_provider(provider_for_order);
    let mut candidates_tried = Vec::new();

    for harness in candidates {
        candidates_tried.push(harness.clone());
        if let Some(route_confidence) =
            candidate_route_confidence(&harness, provider, model_id, installed_harnesses)
        {
            return ProviderCandidateResult {
                selected: Some(SelectedProviderCandidate {
                    harness,
                    route_confidence,
                }),
                candidates_tried,
            };
        }
    }

    ProviderCandidateResult {
        selected: None,
        candidates_tried,
    }
}

fn route_confidence_for_selected_harness(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
) -> RouteConfidence {
    candidate_route_confidence(harness, provider, model_id, installed_harnesses)
        .unwrap_or(RouteConfidence::Passthrough)
}

fn candidate_route_confidence(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
) -> Option<RouteConfidence> {
    if !installed_harnesses.contains(harness) {
        return None;
    }

    if is_native_match(provider, harness) {
        return Some(RouteConfidence::Confirmed);
    }

    if harness == "opencode"
        && opencode_supports_provider_and_model(provider, model_id, installed_harnesses)
    {
        return Some(RouteConfidence::Likely);
    }

    if matches!(harness, "pi" | "cursor") {
        return Some(RouteConfidence::Passthrough);
    }

    None
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
) -> bool {
    let Some(provider) = provider else {
        return false;
    };

    let cached_probe = opencode_cache::read_cached_probe_result();
    matches!(
        crate::models::availability::classify_for_harness(
            "opencode",
            provider,
            model_id,
            installed_harnesses,
            cached_probe.as_ref(),
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

fn normalize_harness_name(value: &str) -> Option<&'static str> {
    match value.trim() {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "cursor" => Some("cursor"),
        "pi" => Some("pi"),
        _ => None,
    }
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
