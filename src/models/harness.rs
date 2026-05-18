// qa-validated: harness-order-settings-audit

use std::collections::HashSet;

const HARNESS_BINARIES: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("cursor", "cursor"),
    ("pi", "pi"),
];

const ORDERABLE_HARNESSES: &[&str] = &["claude", "codex", "opencode", "cursor", "pi"];

pub const VALID_HARNESSES: &[&str] = &["claude", "codex", "pi", "opencode", "cursor"];

pub fn detect_installed_harnesses() -> HashSet<String> {
    HARNESS_BINARIES
        .iter()
        .filter(|(_, binary)| harness_binary_exists(binary))
        .map(|(name, _)| name.to_string())
        .collect()
}

fn harness_binary_exists(binary: &str) -> bool {
    if which::which(binary).is_ok() {
        return true;
    }

    #[cfg(windows)]
    {
        ["exe", "cmd", "bat"]
            .iter()
            .any(|ext| which::which(format!("{binary}.{ext}")).is_ok())
    }

    #[cfg(not(windows))]
    {
        false
    }
}

const PROVIDER_HARNESS_PREFERENCES: &[(&str, &[&str])] = &[
    ("anthropic", &["claude", "pi", "opencode", "cursor"]),
    ("openai", &["codex", "pi", "opencode", "cursor"]),
    ("google", &["pi", "opencode", "cursor"]),
    ("meta", &["pi", "opencode", "cursor"]),
    ("mistral", &["pi", "opencode", "cursor"]),
    ("deepseek", &["pi", "opencode", "cursor"]),
    ("cohere", &["pi", "opencode", "cursor"]),
];

const DEFAULT_FALLBACK_ORDER: &[&str] = &["pi", "opencode", "cursor"];

pub fn is_valid_harness(name: &str) -> bool {
    normalize_harness_name(name).is_some()
}

pub fn normalize_harness_name(name: &str) -> Option<String> {
    let normalized = name.trim().to_ascii_lowercase();
    VALID_HARNESSES
        .contains(&normalized.as_str())
        .then_some(normalized)
}

fn harness_preferences(provider: &str) -> &'static [&'static str] {
    let provider_lower = provider.to_ascii_lowercase();
    PROVIDER_HARNESS_PREFERENCES
        .iter()
        .find(|(p, _)| *p == provider_lower)
        .map(|(_, prefs)| *prefs)
        .unwrap_or(DEFAULT_FALLBACK_ORDER)
}

fn is_launch_bundle_harness(harness: &str) -> bool {
    ORDERABLE_HARNESSES.contains(&harness)
}

pub fn resolve_harness_for_provider(provider: &str, installed: &HashSet<String>) -> Option<String> {
    harness_preferences(provider)
        .iter()
        .find(|h| installed.contains(**h))
        .map(|h| h.to_string())
}

pub fn harness_candidates_for_provider(provider: &str) -> Vec<String> {
    harness_preferences(provider)
        .iter()
        .map(|h| h.to_string())
        .collect()
}

pub fn preferred_harness_for_provider(provider: &str) -> Option<String> {
    harness_candidates_for_provider(provider).into_iter().next()
}

pub struct HarnessCandidateResolution {
    pub harness: Option<String>,
    pub source: Option<&'static str>,
    pub harness_order_position: Option<usize>,
    pub warnings: Vec<String>,
    pub harness_order_failure: Option<HarnessOrderFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessOrderFailure {
    Empty,
    NoneInstalled { valid_candidates: Vec<String> },
}

pub struct ParsedHarnessOrder {
    pub valid_candidates: Vec<String>,
    pub warnings: Vec<String>,
    pub failure: Option<HarnessOrderFailure>,
}

pub fn parse_settings_harness_order(order: &[String]) -> ParsedHarnessOrder {
    if order.is_empty() {
        return ParsedHarnessOrder {
            valid_candidates: Vec::new(),
            warnings: Vec::new(),
            failure: Some(HarnessOrderFailure::Empty),
        };
    }

    let mut valid_candidates = Vec::new();
    let mut warnings = Vec::new();
    for candidate in order {
        let Some(normalized) = normalize_harness_name(candidate) else {
            warnings.push(format!(
                "settings.harness_order contains unrecognized harness `{candidate}`; skipping (valid: {})",
                ORDERABLE_HARNESSES.join(", ")
            ));
            continue;
        };
        if !ORDERABLE_HARNESSES.contains(&normalized.as_str()) {
            warnings.push(format!(
                "settings.harness_order contains unrecognized harness `{candidate}`; skipping (valid: {})",
                ORDERABLE_HARNESSES.join(", ")
            ));
            continue;
        }
        valid_candidates.push(normalized);
    }

    ParsedHarnessOrder {
        valid_candidates,
        warnings,
        failure: None,
    }
}

pub fn resolve_harness_from_candidates(
    provider: Option<&str>,
    settings_harness_order: Option<&[String]>,
    installed: &HashSet<String>,
) -> HarnessCandidateResolution {
    let mut warnings = Vec::new();

    if let Some(order) = settings_harness_order {
        let parsed = parse_settings_harness_order(order);
        warnings.extend(parsed.warnings);
        if parsed.failure == Some(HarnessOrderFailure::Empty) {
            return HarnessCandidateResolution {
                harness: None,
                source: None,
                harness_order_position: None,
                warnings,
                harness_order_failure: Some(HarnessOrderFailure::Empty),
            };
        }

        for (index, normalized) in parsed.valid_candidates.iter().enumerate() {
            if installed.contains(normalized) {
                return HarnessCandidateResolution {
                    harness: Some(normalized.clone()),
                    source: Some("config-order"),
                    harness_order_position: Some(index),
                    warnings,
                    harness_order_failure: None,
                };
            }
        }
        return HarnessCandidateResolution {
            harness: None,
            source: None,
            harness_order_position: None,
            warnings,
            harness_order_failure: (!parsed.valid_candidates.is_empty()).then_some(
                HarnessOrderFailure::NoneInstalled {
                    valid_candidates: parsed.valid_candidates,
                },
            ),
        };
    }

    let harness =
        provider.and_then(|value| resolve_launch_bundle_harness_for_provider(value, installed));
    HarnessCandidateResolution {
        source: harness.as_ref().map(|_| "provider"),
        harness,
        harness_order_position: None,
        warnings,
        harness_order_failure: None,
    }
}

fn resolve_launch_bundle_harness_for_provider(
    provider: &str,
    installed: &HashSet<String>,
) -> Option<String> {
    harness_preferences(provider)
        .iter()
        .find(|h| is_launch_bundle_harness(h) && installed.contains(**h))
        .map(|h| h.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_harness_anthropic_with_claude() {
        let installed: HashSet<String> = ["claude"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resolve_harness_for_provider("anthropic", &installed),
            Some("claude".to_string())
        );
    }

    #[test]
    fn resolve_harness_anthropic_falls_back_to_opencode() {
        let installed: HashSet<String> = ["pi", "opencode"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resolve_harness_for_provider("anthropic", &installed),
            Some("pi".to_string())
        );
    }

    #[test]
    fn resolve_harness_none_installed() {
        let installed: HashSet<String> = HashSet::new();
        assert_eq!(resolve_harness_for_provider("anthropic", &installed), None);
    }

    #[test]
    fn resolve_harness_unknown_provider() {
        let installed: HashSet<String> = ["claude"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resolve_harness_for_provider("unknown-provider", &installed),
            None
        );
    }

    #[test]
    fn resolve_harness_unknown_provider_uses_default_fallback_order() {
        let installed: HashSet<String> = ["opencode", "cursor"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            resolve_harness_for_provider("unknown-provider", &installed),
            Some("opencode".to_string())
        );
    }

    #[test]
    fn resolve_harness_case_insensitive_provider() {
        let installed: HashSet<String> = ["claude"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            resolve_harness_for_provider("Anthropic", &installed),
            Some("claude".to_string())
        );
    }

    #[test]
    fn candidates_for_known_provider() {
        let candidates = harness_candidates_for_provider("openai");
        assert_eq!(candidates, vec!["codex", "pi", "opencode", "cursor"]);
    }

    #[test]
    fn candidates_for_anthropic_use_pi_first_fallback_chain() {
        let candidates = harness_candidates_for_provider("anthropic");
        assert_eq!(candidates, vec!["claude", "pi", "opencode", "cursor"]);
    }

    #[test]
    fn candidates_for_unknown_provider() {
        let candidates = harness_candidates_for_provider("unknown");
        assert_eq!(candidates, vec!["pi", "opencode", "cursor"]);
    }

    #[test]
    fn preferred_harness_for_provider_uses_first_candidate() {
        assert_eq!(
            preferred_harness_for_provider("openai"),
            Some("codex".to_string())
        );
        assert_eq!(
            preferred_harness_for_provider("unknown"),
            Some("pi".to_string())
        );
    }

    #[test]
    fn valid_harness_validation_rejects_gemini() {
        assert!(is_valid_harness("claude"));
        assert!(is_valid_harness("OpenCode"));
        assert!(!is_valid_harness("gemini"));
        assert!(!is_valid_harness("unknown"));
    }

    #[test]
    fn resolve_harness_from_order_selects_first_installed_and_sets_source() {
        let installed: HashSet<String> = ["opencode"].iter().map(|s| s.to_string()).collect();
        let order = vec![
            "pi".to_string(),
            "opencode".to_string(),
            "codex".to_string(),
        ];
        let resolved = resolve_harness_from_candidates(None, Some(&order), &installed);
        assert_eq!(resolved.harness, Some("opencode".to_string()));
        assert_eq!(resolved.source, Some("config-order"));
        assert_eq!(resolved.harness_order_position, Some(1));
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn resolve_harness_from_order_warns_for_unrecognized_entries() {
        let installed: HashSet<String> = ["codex"].iter().map(|s| s.to_string()).collect();
        let order = vec!["unknown-harness".to_string(), "codex".to_string()];
        let resolved = resolve_harness_from_candidates(None, Some(&order), &installed);
        assert_eq!(resolved.harness, Some("codex".to_string()));
        assert_eq!(resolved.source, Some("config-order"));
        assert_eq!(resolved.harness_order_position, Some(0));
        assert!(resolved.warnings.iter().any(|warning| {
            warning.contains("settings.harness_order contains unrecognized harness")
        }));
    }

    #[test]
    fn resolve_harness_from_order_all_invalid() {
        let installed: HashSet<String> = ["codex"].iter().map(|s| s.to_string()).collect();
        let order = vec!["bogus".to_string(), "also-bogus".to_string()];
        let resolved = resolve_harness_from_candidates(None, Some(&order), &installed);
        assert_eq!(resolved.harness, None);
        assert_eq!(resolved.source, None);
        assert_eq!(resolved.harness_order_position, None);
        assert_eq!(resolved.harness_order_failure, None);
        assert_eq!(resolved.warnings.len(), 2);
        assert!(resolved.warnings.iter().all(|warning| {
            warning.contains("settings.harness_order contains unrecognized harness")
        }));
    }

    #[test]
    fn resolve_harness_from_order_warns_for_empty_order() {
        let installed: HashSet<String> = ["codex"].iter().map(|s| s.to_string()).collect();
        let order: Vec<String> = Vec::new();
        let resolved = resolve_harness_from_candidates(None, Some(&order), &installed);
        assert_eq!(resolved.harness, None);
        assert_eq!(resolved.source, None);
        assert_eq!(
            resolved.harness_order_failure,
            Some(HarnessOrderFailure::Empty)
        );
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn resolve_harness_from_order_warns_when_none_installed() {
        let installed: HashSet<String> = ["claude"].iter().map(|s| s.to_string()).collect();
        let order = vec!["pi".to_string(), "opencode".to_string()];
        let resolved = resolve_harness_from_candidates(None, Some(&order), &installed);
        assert_eq!(resolved.harness, None);
        assert_eq!(resolved.source, None);
        assert_eq!(
            resolved.harness_order_failure,
            Some(HarnessOrderFailure::NoneInstalled {
                valid_candidates: vec!["pi".to_string(), "opencode".to_string()],
            })
        );
        assert!(resolved.warnings.is_empty());
    }

    #[test]
    fn resolve_harness_from_candidates_uses_provider_when_order_unset() {
        let installed: HashSet<String> = ["opencode"].iter().map(|s| s.to_string()).collect();
        let resolved = resolve_harness_from_candidates(Some("openai"), None, &installed);
        assert_eq!(resolved.harness, Some("opencode".to_string()));
        assert_eq!(resolved.source, Some("provider"));
        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.harness_order_failure, None);
    }

    #[test]
    fn resolve_harness_from_candidates_provider_skips_non_launch_bundle_harnesses() {
        let installed: HashSet<String> = ["gemini"].iter().map(|s| s.to_string()).collect();
        let resolved = resolve_harness_from_candidates(Some("anthropic"), None, &installed);
        assert_eq!(resolved.harness, None);
        assert_eq!(resolved.source, None);
        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.harness_order_failure, None);
    }
}
