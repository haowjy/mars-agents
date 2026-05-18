use std::collections::HashSet;

use crate::build::policy::PolicyInput;
use crate::compiler::agents::HarnessKind;
use crate::error::{ConfigError, MarsError};
use crate::models;
use crate::models::ModelAlias;
use crate::models::harness::HarnessOrderFailure;

pub(super) struct HarnessResolution {
    pub(super) harness: String,
    pub(super) source: &'static str,
    pub(super) harness_order_position: Option<usize>,
    pub(super) is_experimental: bool,
    pub(super) resolved_harness: HarnessKind,
    pub(super) warnings: Vec<String>,
}

struct CandidateHarnessResolution {
    harness: String,
    source: &'static str,
    harness_order_position: Option<usize>,
    warnings: Vec<String>,
}

pub(super) fn resolve_harness(
    input: &PolicyInput<'_>,
    alias: Option<&ModelAlias>,
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
    let (harness, harness_source) = if let Some(harness) = input.harness_override {
        (harness.to_string(), "cli")
    } else if model_from_cli {
        if let Some(harness) = alias_harness {
            (harness.to_string(), "alias")
        } else {
            let resolved = resolve_harness_candidate_or_fallback(
                provider,
                harness_order,
                &installed_harnesses,
                normalized_config_default_harness.clone(),
            );
            selected_harness_order_position = resolved.harness_order_position;
            warnings.extend(resolved.warnings);
            (resolved.harness, resolved.source)
        }
    } else if let Some(harness) = profile_harness {
        (harness.to_string(), "profile")
    } else if let Some(harness) = alias_harness {
        (harness.to_string(), "alias")
    } else {
        let resolved = resolve_harness_candidate_or_fallback(
            provider,
            harness_order,
            &installed_harnesses,
            normalized_config_default_harness,
        );
        selected_harness_order_position = resolved.harness_order_position;
        warnings.extend(resolved.warnings);
        (resolved.harness, resolved.source)
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
    provider: Option<&str>,
    settings_harness_order: Option<&[String]>,
    installed_harnesses: &HashSet<String>,
    config_default_harness: Option<String>,
) -> CandidateHarnessResolution {
    let mut candidate = models::harness::resolve_harness_from_candidates(
        provider,
        settings_harness_order,
        installed_harnesses,
    );
    if let Some(harness) = candidate.harness {
        return CandidateHarnessResolution {
            harness,
            source: candidate.source.unwrap_or("provider"),
            harness_order_position: candidate.harness_order_position,
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
        warnings,
    }
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
