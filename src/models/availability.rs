use std::collections::HashSet;

use serde::Serialize;

use super::probes::OpenCodeProbeResult;

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
    #[serde(rename = "opencode_probe")]
    OpenCodeProbe,
    #[serde(rename = "opencode_probe_negative")]
    OpenCodeProbeNegative,
    #[serde(rename = "opencode_probe_unknown")]
    OpenCodeProbeUnknown,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecomposedSlug {
    pub oc_provider: String,
    pub upstream_provider: Option<String>,
    pub model_part: String,
    pub full_slug: String,
}

pub fn decompose_slug(slug: &str) -> Option<DecomposedSlug> {
    let parts: Vec<&str> = slug.split('/').collect();
    match parts.as_slice() {
        [oc_provider, model_part] if !oc_provider.is_empty() && !model_part.is_empty() => {
            Some(DecomposedSlug {
                oc_provider: (*oc_provider).to_string(),
                upstream_provider: None,
                model_part: (*model_part).to_string(),
                full_slug: slug.to_string(),
            })
        }
        [oc_provider, upstream_provider, model_part]
            if !oc_provider.is_empty()
                && !upstream_provider.is_empty()
                && !model_part.is_empty() =>
        {
            Some(DecomposedSlug {
                oc_provider: (*oc_provider).to_string(),
                upstream_provider: Some((*upstream_provider).to_string()),
                model_part: (*model_part).to_string(),
                full_slug: slug.to_string(),
            })
        }
        _ => None,
    }
}

pub fn normalize_model_id(id: &str) -> String {
    id.to_lowercase().replace('.', "-")
}

pub fn model_id_matches(mars_id: &str, oc_model: &str) -> bool {
    normalize_model_id(mars_id) == normalize_model_id(oc_model)
}

pub fn provider_matches(mars_provider: &str, oc_segment: &str) -> bool {
    mars_provider.eq_ignore_ascii_case(oc_segment)
}

/// Classify availability for a model through a specific harness.
pub fn classify_for_harness(
    harness: &str,
    provider: &str,
    model_id: &str,
    installed: &HashSet<String>,
    probe_result: Option<&OpenCodeProbeResult>,
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
        "claude" => provider_matches(provider, "anthropic"),
        "codex" => provider_matches(provider, "openai"),
        "gemini" => provider_matches(provider, "google"),
        "opencode" => return classify_opencode(provider, model_id, probe_result),
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

    if !probe.provider_probe_success {
        return Some((
            AvailabilityStatus::Unknown,
            AvailabilitySource::OpenCodeProbeUnknown,
            None,
        ));
    }

    let provider_lower = provider.to_lowercase();
    let has_provider = probe
        .providers
        .get(&provider_lower)
        .copied()
        .unwrap_or(false);
    let has_openrouter = probe.providers.get("openrouter").copied().unwrap_or(false);
    let has_via_openrouter = has_openrouter && openrouter_supports_provider(&provider_lower);

    if !has_provider && !has_via_openrouter {
        return Some((
            AvailabilityStatus::Unavailable,
            AvailabilitySource::OpenCodeProbeNegative,
            None,
        ));
    }

    if probe.model_probe_success {
        let Some(harness_model_id) = find_matching_slug(model_id, provider, &probe.model_slugs)
        else {
            return Some((
                AvailabilityStatus::Unavailable,
                AvailabilitySource::OpenCodeProbeNegative,
                None,
            ));
        };

        return Some((
            AvailabilityStatus::Runnable,
            AvailabilitySource::OpenCodeProbe,
            Some(RunnablePath {
                harness: "opencode".to_string(),
                mars_provider: provider.to_string(),
                harness_model_id,
            }),
        ));
    }

    let harness_model_id = if has_via_openrouter && !has_provider {
        format!("openrouter/{provider_lower}/{model_id}")
    } else {
        format!("{provider_lower}/{model_id}")
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

fn openrouter_supports_provider(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic" | "meta" | "mistral" | "deepseek" | "cohere"
    )
}

fn find_matching_slug(
    mars_model_id: &str,
    mars_provider: &str,
    slugs: &[String],
) -> Option<String> {
    for slug in slugs {
        let Some(decomposed) = decompose_slug(slug) else {
            continue;
        };
        let effective_provider = decomposed
            .upstream_provider
            .as_deref()
            .unwrap_or(&decomposed.oc_provider);

        if provider_matches(mars_provider, effective_provider)
            && model_id_matches(mars_model_id, &decomposed.model_part)
        {
            return Some(slug.clone());
        }
    }

    None
}

pub fn classify_model(
    model_id: &str,
    provider: &str,
    installed: &HashSet<String>,
    probe_result: Option<&OpenCodeProbeResult>,
    offline: bool,
) -> ModelAvailability {
    let mut statuses = Vec::new();
    let mut runnable_paths = Vec::new();

    for harness in ["claude", "codex", "gemini"] {
        let Some((status, source, path)) =
            classify_for_harness(harness, provider, model_id, installed, None)
        else {
            continue;
        };
        if let Some(path) = path {
            runnable_paths.push(path);
        }
        statuses.push((status, source));
    }

    if installed.contains("opencode") {
        if offline {
            statuses.push((AvailabilityStatus::Unknown, AvailabilitySource::Offline));
        } else if let Some(result) = probe_result {
            if let Some((status, source, path)) =
                classify_for_harness("opencode", provider, model_id, installed, Some(result))
            {
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

    aggregate_statuses(statuses, runnable_paths)
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
    use std::collections::HashMap;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn test_decompose_slug_two_segments() {
        let slug = decompose_slug("openai/gpt-5.4").unwrap();
        assert_eq!(slug.oc_provider, "openai");
        assert_eq!(slug.upstream_provider, None);
        assert_eq!(slug.model_part, "gpt-5.4");
        assert_eq!(slug.full_slug, "openai/gpt-5.4");
    }

    #[test]
    fn test_decompose_slug_three_segments() {
        let slug = decompose_slug("openrouter/anthropic/claude-opus-4.7").unwrap();
        assert_eq!(slug.oc_provider, "openrouter");
        assert_eq!(slug.upstream_provider.as_deref(), Some("anthropic"));
        assert_eq!(slug.model_part, "claude-opus-4.7");
        assert_eq!(slug.full_slug, "openrouter/anthropic/claude-opus-4.7");
    }

    #[test]
    fn test_decompose_slug_invalid() {
        assert!(decompose_slug("gpt-5").is_none());
        assert!(decompose_slug("openai/").is_none());
        assert!(decompose_slug("a/b/c/d").is_none());
    }

    #[test]
    fn test_normalize_model_id() {
        assert_eq!(normalize_model_id("Claude-Opus-4.7"), "claude-opus-4-7");
    }

    #[test]
    fn test_model_id_matches() {
        assert!(model_id_matches("claude-opus-4-7", "Claude-Opus-4.7"));
        assert!(!model_id_matches("claude-opus-4-7", "claude-sonnet-4-7"));
    }

    #[test]
    fn test_provider_matches() {
        assert!(provider_matches("Anthropic", "anthropic"));
        assert!(!provider_matches("Anthropic", "openai"));
    }

    #[test]
    fn test_classify_claude_anthropic() {
        let result = classify_for_harness(
            "claude",
            "Anthropic",
            "claude-opus-4-7",
            &installed(&["claude"]),
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
        let result =
            classify_for_harness("codex", "OpenAI", "gpt-5.4", &installed(&["codex"]), None)
                .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Runnable);
        assert_eq!(result.1, AvailabilitySource::HarnessInstalled);
    }

    #[test]
    fn test_classify_gemini_google() {
        let result = classify_for_harness(
            "gemini",
            "Google",
            "gemini-2.5-pro",
            &installed(&["gemini"]),
            None,
        )
        .unwrap();
        assert_eq!(result.0, AvailabilityStatus::Runnable);
        assert_eq!(result.1, AvailabilitySource::HarnessInstalled);
    }

    #[test]
    fn test_classify_no_harness() {
        let result = classify_for_harness(
            "claude",
            "Anthropic",
            "claude-opus-4-7",
            &installed(&[]),
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
            false,
        );
        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::HarnessInstalled);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "claude");
    }

    #[test]
    fn test_classify_multi_harness_all_unavailable() {
        let result = classify_model("custom-model", "Unknown", &installed(&[]), None, false);
        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::NoHarness);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_offline_mode() {
        let result = classify_model("gpt-5.4", "OpenAI", &installed(&["codex"]), None, true);
        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::HarnessInstalled);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "codex");

        let result = classify_model("gpt-5.4", "OpenAI", &installed(&["opencode"]), None, true);
        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::Offline);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_direct_slug() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("openai".to_string(), true)]),
            model_slugs: vec!["openai/gpt-5.4".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbe);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(result.runnable_paths[0].harness, "opencode");
        assert_eq!(result.runnable_paths[0].harness_model_id, "openai/gpt-5.4");
    }

    #[test]
    fn test_classify_opencode_openrouter_slug() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("openrouter".to_string(), true)]),
            model_slugs: vec!["openrouter/anthropic/claude-opus-4.7".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbe);
        assert_eq!(
            result.runnable_paths[0].harness_model_id,
            "openrouter/anthropic/claude-opus-4.7"
        );
    }

    #[test]
    fn test_classify_opencode_provider_negative() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("google".to_string(), true)]),
            provider_probe_success: true,
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_empty_providers() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::new(),
            model_slugs: Vec::new(),
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_no_matching_slug() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("anthropic".to_string(), true)]),
            model_slugs: vec!["anthropic/claude-3-5-sonnet".to_string()],
            provider_probe_success: true,
            model_probe_success: true,
            error: None,
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unavailable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeNegative);
        assert!(result.runnable_paths.is_empty());
    }

    #[test]
    fn test_classify_opencode_synthesizes_slug_when_model_probe_fails() {
        let probe = OpenCodeProbeResult {
            providers: HashMap::from([("anthropic".to_string(), true)]),
            provider_probe_success: true,
            model_probe_success: false,
            error: Some("model probe failed: timeout".to_string()),
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "claude-opus-4-7",
            "Anthropic",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Runnable);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbe);
        assert_eq!(result.runnable_paths.len(), 1);
        assert_eq!(
            result.runnable_paths[0].harness_model_id,
            "anthropic/claude-opus-4-7"
        );
    }

    #[test]
    fn test_classify_opencode_unknown_when_probe_fails() {
        let probe = OpenCodeProbeResult {
            error: Some("provider probe failed: timeout".to_string()),
            ..OpenCodeProbeResult::default()
        };

        let result = classify_model(
            "gpt-5.4",
            "OpenAI",
            &installed(&["opencode"]),
            Some(&probe),
            false,
        );

        assert_eq!(result.status, AvailabilityStatus::Unknown);
        assert_eq!(result.source, AvailabilitySource::OpenCodeProbeUnknown);
        assert!(result.runnable_paths.is_empty());
    }
}
