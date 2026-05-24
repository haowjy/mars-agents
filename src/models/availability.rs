use std::collections::HashSet;

use serde::Serialize;

use crate::routing::slug;

use super::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    Runnable,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilitySource {
    HarnessInstalled,
    UniversalHarness,
    #[serde(rename = "pi_probe")]
    PiProbe,
    #[serde(rename = "pi_probe_negative")]
    PiProbeNegative,
    #[serde(rename = "opencode_probe")]
    OpenCodeProbe,
    #[serde(rename = "opencode_probe_negative")]
    OpenCodeProbeNegative,
    #[serde(rename = "opencode_probe_unknown")]
    OpenCodeProbeUnknown,
    #[serde(rename = "cursor_probe")]
    CursorProbe,
    #[serde(rename = "cursor_probe_negative")]
    CursorProbeNegative,
    #[serde(rename = "cursor_probe_unknown")]
    CursorProbeUnknown,
    NoHarness,
    Offline,
}

/// A runnable model path — one specific way to execute a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnablePath {
    pub harness: String,
    pub mars_provider: String,
    pub harness_model_id: String,
}

/// Full availability assessment for a resolved model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelAvailability {
    pub status: AvailabilityStatus,
    pub source: AvailabilitySource,
    pub runnable_paths: Vec<RunnablePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnablePathSource {
    CachedProbe,
    ProviderMatch,
    Synthesized,
    Passthrough,
}

impl RunnablePathSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::CachedProbe => "cached-probe",
            Self::ProviderMatch => "provider-match",
            Self::Synthesized => "synthesized",
            Self::Passthrough => "passthrough",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableConfidence {
    Confirmed,
    Likely,
    Unknown,
}

impl RunnableConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Likely => "likely",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRunnablePath {
    pub harness_model_id: String,
    pub source: RunnablePathSource,
    pub confidence: RunnableConfidence,
}

pub fn resolve_runnable_path(
    model_id: &str,
    provider: &str,
    target_harness: &str,
    probe_result: Option<&OpenCodeProbeResult>,
) -> ResolvedRunnablePath {
    super::harness_model::resolve_harness_model(super::harness_model::HarnessModelInput {
        harness: target_harness,
        model_id,
        provider_constraint: None,
        provider_for_order: (!provider.trim().is_empty()).then_some(provider),
        settings_provider_order: None,
        opencode_probe: probe_result,
        pi_probe: None,
    })
}

/// Classify availability for a model through a specific harness.
pub fn classify_for_harness(
    harness: &str,
    provider: &str,
    model_id: &str,
    installed: &HashSet<String>,
    probe_result: Option<&OpenCodeProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
) -> Option<(AvailabilityStatus, AvailabilitySource, Option<RunnablePath>)> {
    let harness = harness.to_ascii_lowercase();
    if !installed.contains(&harness) {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::NoHarness,
            None,
        ));
    }

    let direct_match = match harness.as_str() {
        "claude" => slug::providers_match(provider, "anthropic"),
        "codex" => slug::providers_match(provider, "openai"),
        "opencode" => return classify_opencode(provider, model_id, probe_result),
        "pi" => return classify_universal_harness(),
        "cursor" => return classify_cursor(model_id, cursor_probe_result),
        _ => false,
    };

    if direct_match {
        Some((
            AvailabilityStatus::Runnable,
            AvailabilitySource::HarnessInstalled,
            Some(RunnablePath {
                harness,
                mars_provider: provider.to_string(),
                harness_model_id: model_id.to_string(),
            }),
        ))
    } else {
        Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::NoHarness,
            None,
        ))
    }
}

fn classify_universal_harness()
-> Option<(AvailabilityStatus, AvailabilitySource, Option<RunnablePath>)> {
    Some((
        AvailabilityStatus::Unknown,
        AvailabilitySource::UniversalHarness,
        None,
    ))
}

fn classify_opencode(
    provider: &str,
    model_id: &str,
    probe_result: Option<&OpenCodeProbeResult>,
) -> Option<(AvailabilityStatus, AvailabilitySource, Option<RunnablePath>)> {
    let Some(probe) = probe_result else {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::OpenCodeProbeUnknown,
            None,
        ));
    };

    if !probe.model_probe_success {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::OpenCodeProbeUnknown,
            None,
        ));
    }

    if is_unknown_provider(provider) {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::OpenCodeProbeUnknown,
            None,
        ));
    }

    let Some(harness_model_id) = slug::find_exact_match(
        model_id,
        provider,
        probe.model_slugs.iter().map(String::as_str),
    )
    .map(|matched| matched.slug) else {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::OpenCodeProbeNegative,
            None,
        ));
    };

    Some((
        AvailabilityStatus::Runnable,
        AvailabilitySource::OpenCodeProbe,
        Some(RunnablePath {
            harness: "opencode".to_string(),
            mars_provider: provider.to_string(),
            harness_model_id,
        }),
    ))
}

fn classify_cursor(
    model_id: &str,
    probe_result: Option<&CursorProbeResult>,
) -> Option<(AvailabilityStatus, AvailabilitySource, Option<RunnablePath>)> {
    let Some(probe) = probe_result else {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::CursorProbeUnknown,
            None,
        ));
    };

    if !probe.model_probe_success {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::CursorProbeUnknown,
            None,
        ));
    }
    if probe.slugs.is_empty() {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::CursorProbeUnknown,
            None,
        ));
    }

    let matches = crate::models::probes::cursor::find_cursor_prefix_matches(model_id, &probe.slugs);
    if matches.is_empty() {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::CursorProbeNegative,
            None,
        ));
    }

    Some((
        AvailabilityStatus::Runnable,
        AvailabilitySource::CursorProbe,
        Some(RunnablePath {
            harness: "cursor".to_string(),
            mars_provider: "cursor".to_string(),
            harness_model_id: model_id.to_string(),
        }),
    ))
}

fn is_unknown_provider(provider: &str) -> bool {
    let provider = provider.trim();
    provider.is_empty() || provider.eq_ignore_ascii_case("unknown")
}

pub fn classify_model(
    model_id: &str,
    provider: &str,
    installed: &HashSet<String>,
    opencode_probe_result: Option<&OpenCodeProbeResult>,
    pi_probe_result: Option<&PiProbeResult>,
    cursor_probe_result: Option<&CursorProbeResult>,
    offline: bool,
) -> ModelAvailability {
    let mut statuses = Vec::new();
    let mut runnable_paths = Vec::new();

    for harness in ["claude", "codex"] {
        let Some((status, source, path)) =
            classify_for_harness(harness, provider, model_id, installed, None, None)
        else {
            continue;
        };
        if let Some(path) = path {
            runnable_paths.push(path);
        }
        statuses.push((status, source));
    }

    if let Some((status, source, path)) =
        classify_pi_for_model(provider, model_id, installed, pi_probe_result, offline)
    {
        if let Some(path) = path {
            runnable_paths.push(path);
        }
        statuses.push((status, source));
    }

    if installed.contains("opencode") {
        if offline {
            statuses.push((AvailabilityStatus::Unknown, AvailabilitySource::Offline));
        } else if let Some(result) = opencode_probe_result {
            if let Some((status, source, path)) = classify_for_harness(
                "opencode",
                provider,
                model_id,
                installed,
                Some(result),
                None,
            ) {
                if let Some(path) = path {
                    runnable_paths.push(path);
                }
                statuses.push((status, source));
            }
        } else {
            statuses.push((
                AvailabilityStatus::Unknown,
                AvailabilitySource::OpenCodeProbeUnknown,
            ));
        }
    }

    if installed.contains("cursor") {
        if offline {
            statuses.push((AvailabilityStatus::Unknown, AvailabilitySource::Offline));
        } else if let Some((status, source, path)) = classify_cursor(model_id, cursor_probe_result)
        {
            if let Some(path) = path {
                runnable_paths.push(path);
            }
            statuses.push((status, source));
        }
    }

    aggregate_statuses(statuses, runnable_paths)
}

fn classify_pi_for_model(
    provider: &str,
    model_id: &str,
    installed: &HashSet<String>,
    pi_probe_result: Option<&PiProbeResult>,
    offline: bool,
) -> Option<(AvailabilityStatus, AvailabilitySource, Option<RunnablePath>)> {
    if !installed.contains("pi") {
        return None;
    }

    if offline || pi_probe_result.is_none() {
        return classify_universal_harness();
    }

    let pi_probe_result = pi_probe_result.expect("checked is_some above");
    if !pi_probe_result.compatible {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::PiProbeNegative,
            None,
        ));
    }

    let Some(harness_model_id) = slug::find_exact_match(
        model_id,
        provider,
        pi_probe_result.model_slugs.iter().map(String::as_str),
    )
    .map(|matched| matched.slug) else {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::PiProbeNegative,
            None,
        ));
    };

    Some((
        AvailabilityStatus::Runnable,
        AvailabilitySource::PiProbe,
        Some(RunnablePath {
            harness: "pi".to_string(),
            mars_provider: provider.to_string(),
            harness_model_id,
        }),
    ))
}

fn aggregate_statuses(
    statuses: Vec<(AvailabilityStatus, AvailabilitySource)>,
    runnable_paths: Vec<RunnablePath>,
) -> ModelAvailability {
    if statuses.is_empty() {
        return ModelAvailability {
            status: AvailabilityStatus::Unavailable,
            source: AvailabilitySource::NoHarness,
            runnable_paths: Vec::new(),
        };
    }

    if statuses
        .iter()
        .any(|(status, _)| *status == AvailabilityStatus::Runnable)
    {
        return ModelAvailability {
            status: AvailabilityStatus::Runnable,
            source: statuses
                .iter()
                .find_map(|(status, source)| {
                    (*status == AvailabilityStatus::Runnable).then(|| source.clone())
                })
                .expect("runnable status exists"),
            runnable_paths,
        };
    }

    if statuses
        .iter()
        .any(|(status, _)| *status == AvailabilityStatus::Unknown)
    {
        return ModelAvailability {
            status: AvailabilityStatus::Unknown,
            source: statuses
                .iter()
                .find_map(|(status, source)| {
                    (*status == AvailabilityStatus::Unknown).then(|| source.clone())
                })
                .unwrap_or(AvailabilitySource::OpenCodeProbeUnknown),
            runnable_paths: Vec::new(),
        };
    }

    ModelAvailability {
        status: AvailabilityStatus::Unavailable,
        source: statuses
            .iter()
            .find_map(|(_, source)| {
                (*source != AvailabilitySource::NoHarness).then(|| source.clone())
            })
            .or_else(|| statuses.first().map(|(_, source)| source.clone()))
            .unwrap_or(AvailabilitySource::NoHarness),
        runnable_paths: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn test_classify_claude_anthropic() {
        let result = classify_for_harness(
            "claude",
            "Anthropic",
            "claude-opus-4-7",
            &installed(&["claude"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Runnable);
        assert_eq!(result.1, AvailabilitySource::HarnessInstalled);
        assert_eq!(
            result.2.unwrap().harness_model_id,
            "claude-opus-4-7".to_string()
        );
    }

    #[test]
    fn test_classify_codex_openai() {
        let result = classify_for_harness(
            "codex",
            "OpenAI",
            "gpt-5.4",
            &installed(&["codex"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Runnable);
        assert_eq!(result.1, AvailabilitySource::HarnessInstalled);
    }

    #[test]
    fn test_classify_codex_openai_codex_variant_is_runnable() {
        let result = classify_for_harness(
            "codex",
            "openai-codex",
            "gpt-5.4-mini",
            &installed(&["codex"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Runnable);
        assert_eq!(result.1, AvailabilitySource::HarnessInstalled);
        assert_eq!(
            result
                .2
                .expect("runnable path should be present")
                .harness_model_id,
            "gpt-5.4-mini"
        );
    }

    #[test]
    fn test_classify_pi_is_universal_unknown_when_installed() {
        let result = classify_for_harness(
            "pi",
            "OpenAI",
            "gpt-5.4-mini",
            &installed(&["pi"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Unknown);
        assert_eq!(result.1, AvailabilitySource::UniversalHarness);
        assert!(result.2.is_none());
    }

    #[test]
    fn test_classify_cursor_is_universal_unknown_when_installed() {
        let result = classify_for_harness(
            "cursor",
            "Anthropic",
            "claude-opus-4-7",
            &installed(&["cursor"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Unknown);
        assert_eq!(result.1, AvailabilitySource::CursorProbeUnknown);
        assert!(result.2.is_none());
    }

    #[test]
    fn test_classify_cursor_probe_prefix_match_is_runnable() {
        let cursor_probe = CursorProbeResult {
            slugs: vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()],
            model_probe_success: true,
            error: None,
        };
        let result = classify_model(
            "gpt-5.5",
            "OpenAI",
            &installed(&["cursor"]),
            None,
            None,
            Some(&cursor_probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::CursorProbe);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "cursor");
        assert_eq!(result.runnable_paths[0].harness_model_id, "gpt-5.5");
    }

    #[test]
    fn test_classify_cursor_probe_no_match_is_unavailable() {
        let cursor_probe = CursorProbeResult {
            slugs: vec!["claude-opus-4-7-high".to_string()],
            model_probe_success: true,
            error: None,
        };
        let result = classify_model(
            "gpt-5.5",
            "OpenAI",
            &installed(&["cursor"]),
            None,
            None,
            Some(&cursor_probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::CursorProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_cursor_probe_empty_catalog_is_unknown() {
        let cursor_probe = CursorProbeResult {
            slugs: Vec::new(),
            model_probe_success: true,
            error: None,
        };
        let result = classify_model(
            "gpt-5.5",
            "OpenAI",
            &installed(&["cursor"]),
            None,
            None,
            Some(&cursor_probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::CursorProbeUnknown);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_no_harness() {
        let result = classify_for_harness(
            "claude",
            "Anthropic",
            "claude-opus-4-7",
            &installed(&[]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Unavailable);
        assert_eq!(result.1, AvailabilitySource::NoHarness);
        assert!(result.2.is_none());
    }

    #[test]
    fn test_classify_multi_harness_any_runnable() {
        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["claude", "codex"]),
            None,
            None,
            None,
            false,
        );
        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::HarnessInstalled);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "claude");
    }

    #[test]
    fn test_classify_multi_harness_all_unavailable() {
        let result = classify_model(
            "custom-model",
            "Unknown",
            &installed(&[]),
            None,
            None,
            None,
            false,
        );
        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::NoHarness);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_google_model_with_only_pi_installed_is_unknown_universal() {
        let result = classify_model(
            "gemini-2.5-pro",
            "Google",
            &installed(&["pi"]),
            None,
            None,
            None,
            false,
        );
        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::UniversalHarness);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_pi_probe_compatible_is_runnable() {
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai/gpt-5.4-mini".to_string()]),
            ..PiProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4-mini",
            "OpenAI",
            &installed(&["pi"]),
            None,
            Some(&pi_probe),
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::PiProbe);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "pi");
        assert_eq!(
            result.runnable_paths[0].harness_model_id,
            "openai/gpt-5.4-mini"
        );
    }

    #[test]
    fn test_classify_pi_probe_incompatible_is_unavailable_without_other_harnesses() {
        let pi_probe = PiProbeResult {
            compatible: false,
            ..PiProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4-mini",
            "OpenAI",
            &installed(&["pi"]),
            None,
            Some(&pi_probe),
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::PiProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_pi_probe_incompatible_yields_to_runnable_harness() {
        let pi_probe = PiProbeResult {
            compatible: false,
            ..PiProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4-mini",
            "OpenAI",
            &installed(&["pi", "codex"]),
            None,
            Some(&pi_probe),
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::HarnessInstalled);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "codex");
    }

    #[test]
    fn test_classify_pi_probe_missing_model_is_unavailable() {
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai/gpt-5.4".to_string()]),
            ..PiProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4-mini",
            "OpenAI",
            &installed(&["pi"]),
            None,
            Some(&pi_probe),
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::PiProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_offline_mode() {
        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["codex"]),
            None,
            None,
            None,
            true,
        );
        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::HarnessInstalled);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "codex");

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            None,
            None,
            None,
            true,
        );
        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::Offline);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_direct_slug() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4".to_string()],
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbe);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "opencode");
        assert_eq!(result.runnable_paths[0].harness_model_id, "openai/gpt-5.4");
    }

    #[test]
    fn test_classify_opencode_nested_provider_slug_is_not_flattened() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openrouter/anthropic/claude-opus-4.7".to_string()],
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_provider_negative() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["google/gemini-2.5-pro".to_string()],
            model_probe_success: true,
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_empty_slugs() {
        let probe = OpenCodeProbeResult {
            model_slugs: Vec::new(),
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_no_matching_slug() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["anthropic/claude-3-5-sonnet".to_string()],
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_unknown_when_model_probe_fails() {
        let probe = OpenCodeProbeResult {
            model_probe_success: false,
            error: Some("model probe failed: timeout".to_string()),
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeUnknown);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_resolve_runnable_path_prefers_cached_probe_slug() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4".to_string()],
            model_probe_success: true,
            error: None,
        };

        let resolved = resolve_runnable_path("gpt-5.4", "OpenAI", "opencode", Some(&probe));
        assert_eq!(resolved.harness_model_id, "openai/gpt-5.4");
        assert_eq!(resolved.source, RunnablePathSource::CachedProbe);
        assert_eq!(resolved.confidence, RunnableConfidence::Confirmed);
    }

    #[test]
    fn test_resolve_runnable_path_falls_back_to_passthrough_without_slug_match() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openrouter/anthropic/claude-sonnet-4-7".to_string()],
            model_probe_success: true,
            error: None,
        };

        let resolved =
            resolve_runnable_path("claude-opus-4-7", "Anthropic", "opencode", Some(&probe));
        assert_eq!(resolved.harness_model_id, "claude-opus-4-7");
        assert_eq!(resolved.source, RunnablePathSource::Passthrough);
        assert_eq!(resolved.confidence, RunnableConfidence::Unknown);
    }

    #[test]
    fn test_classify_opencode_unknown_when_probe_fails() {
        let probe = OpenCodeProbeResult {
            error: Some("model probe failed: timeout".to_string()),
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeUnknown);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_unknown_provider_stays_unknown() {
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4".to_string()],
            model_probe_success: true,
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "mystery-model",
            "unknown",
            &installed(&["opencode"]),
            Some(&probe),
            None,
            None,
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeUnknown);
        assert!(result.runnable_paths.is_empty());
    }
}
