use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models;
use crate::models::ModelAlias;
use crate::models::availability::AvailabilityStatus;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::OpenCodeProbeResult;
use wait_timeout::ChildExt;

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
    config_default_harness: Option<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
) -> CandidateHarnessResolution {
    let mut warnings = Vec::new();
    let mut candidates_tried = Vec::new();
    let mut harness_order_failure = None;

    let candidates = if let Some(order) = settings_harness_order {
        let parsed_order = models::harness::parse_settings_harness_order(order);
        warnings.extend(parsed_order.warnings);
        harness_order_failure = parsed_order.failure;
        if harness_order_failure.is_none()
            && !parsed_order.valid_candidates.is_empty()
            && parsed_order
                .valid_candidates
                .iter()
                .all(|candidate| !installed_harnesses.contains(candidate))
        {
            harness_order_failure = Some(HarnessOrderFailure::NoneInstalled {
                valid_candidates: parsed_order.valid_candidates.clone(),
            });
        }
        parsed_order
            .valid_candidates
            .into_iter()
            .enumerate()
            .map(|(index, harness)| (harness, Some(index)))
            .collect::<Vec<_>>()
    } else {
        let provider_for_order = provider.unwrap_or("unknown");
        models::harness::harness_candidates_for_provider(provider_for_order)
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

fn candidate_route_confidence(
    harness: &str,
    provider: Option<&str>,
    model_id: &str,
    installed_harnesses: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
) -> Option<RouteConfidence> {
    if !installed_harnesses.contains(harness) {
        return None;
    }

    if is_native_match(provider, harness) && native_harness_authenticated(harness) {
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
    match harness {
        "codex" => run_auth_status_command("codex", &["login", "status"]),
        "claude" => run_auth_status_command("claude", &["auth", "status"]),
        _ => false,
    }
}

fn run_auth_status_command(command: &str, args: &[&str]) -> bool {
    let mut child = match Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    match child.wait_timeout(auth_probe_timeout()) {
        Ok(Some(status)) => status.success(),
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        Err(_) => false,
    }
}

fn auth_probe_timeout() -> Duration {
    std::env::var("MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(2))
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
