use std::collections::BTreeSet;
use std::path::Path;

use crate::harness::registry::HarnessId;

use super::Settings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingConfigDiagnostic {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoutingSettings {
    pub harness_order: Option<ParsedHarnessOrder>,
    pub provider_order: Option<ParsedProviderOrder>,
    pub default_harness: Option<ParsedHarnessValue>,
    pub linked_harnesses: BTreeSet<HarnessId>,
    pub diagnostics: Vec<RoutingConfigDiagnostic>,
}

impl ResolvedRoutingSettings {
    /// Resolve routing settings from mars.toml. Falls back to Settings::default()
    /// if mars.toml cannot be loaded.
    pub fn from_config(root: &Path) -> Self {
        match crate::config::load(root) {
            Ok(config) => resolve(&config.settings),
            Err(_) => resolve(&Settings::default()),
        }
    }

    pub fn harness_order_names(&self) -> Option<Vec<String>> {
        self.harness_order.as_ref().map(|order| {
            order
                .candidates
                .iter()
                .map(|c| c.harness.to_string())
                .collect()
        })
    }

    pub fn default_harness_name(&self) -> Option<String> {
        self.default_harness
            .as_ref()
            .map(|value| value.harness.to_string())
    }

    pub fn provider_order_names(&self) -> Option<Vec<String>> {
        self.provider_order
            .as_ref()
            .map(|order| order.providers.clone())
    }

    pub fn linked_harness_names(&self) -> Vec<String> {
        self.linked_harnesses
            .iter()
            .map(|harness| harness.to_string())
            .collect()
    }

    pub fn diagnostic_messages(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHarnessValue {
    pub harness: HarnessId,
    pub original: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHarnessOrder {
    pub candidates: Vec<OrderedHarnessCandidate>,
    pub failure: Option<HarnessOrderFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedProviderOrder {
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessOrderFailure {
    Empty,
    AllInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedHarnessCandidate {
    pub harness: HarnessId,
    pub original: String,
    pub position: usize,
}

pub fn resolve(settings: &Settings) -> ResolvedRoutingSettings {
    let mut diagnostics = Vec::new();

    let harness_order = settings.harness_order.as_ref().map(|order| {
        if order.is_empty() {
            diagnostics.push(RoutingConfigDiagnostic {
                message:
                    "settings.harness_order is empty; falling through to provider candidate order"
                        .to_string(),
            });
            return ParsedHarnessOrder {
                candidates: Vec::new(),
                failure: Some(HarnessOrderFailure::Empty),
            };
        }

        let mut candidates = Vec::new();
        for (position, candidate) in order.iter().enumerate() {
            let Some(harness) = crate::harness::registry::parse(candidate) else {
                diagnostics.push(RoutingConfigDiagnostic {
                    message: format!(
                        "settings.harness_order contains unrecognized harness `{candidate}`; skipping (valid: {})",
                        crate::harness::registry::names().join(", ")
                    ),
                });
                continue;
            };
            candidates.push(OrderedHarnessCandidate {
                harness,
                original: candidate.clone(),
                position,
            });
        }

        let failure = if candidates.is_empty() {
            diagnostics.push(RoutingConfigDiagnostic {
                message:
                    "settings.harness_order has no valid candidates; falling through to provider candidate order"
                        .to_string(),
            });
            Some(HarnessOrderFailure::AllInvalid)
        } else {
            None
        };

        ParsedHarnessOrder {
            candidates,
            failure,
        }
    });

    let default_harness = settings.default_harness.as_ref().and_then(|value| {
        match crate::harness::registry::parse(value) {
            Some(harness) => Some(ParsedHarnessValue {
                harness,
                original: value.clone(),
            }),
            None => {
                diagnostics.push(RoutingConfigDiagnostic {
                    message: format!(
                        "settings.default_harness `{value}` is invalid; expected one of: {}",
                        crate::harness::registry::names().join(", ")
                    ),
                });
                None
            }
        }
    });

    let provider_order = settings.provider_order.as_ref().map(|order| ParsedProviderOrder {
        providers: order
            .iter()
            .filter_map(|provider| {
                let normalized = provider.trim().to_ascii_lowercase();
                if normalized.is_empty() {
                    return None;
                }
                if !is_known_provider_or_variant(&normalized) {
                    diagnostics.push(RoutingConfigDiagnostic {
                        message: format!(
                            "settings.provider_order contains unknown provider `{provider}`; keeping it for forward-compat routing preferences"
                        ),
                    });
                }
                Some(normalized)
            })
            .collect(),
    });

    let linked_harnesses = settings.effective_links().linked_harnesses_set();

    ResolvedRoutingSettings {
        harness_order,
        provider_order,
        default_harness,
        linked_harnesses,
        diagnostics,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn settings_with_links(targets: Option<Vec<&str>>) -> Settings {
        Settings {
            managed_root: None,
            targets: targets.map(|values| values.into_iter().map(str::to_string).collect()),
            ..Settings::default()
        }
    }

    #[test]
    fn parse_invalid_default_harness_to_diagnostic() {
        let mut settings = settings_with_links(None);
        settings.default_harness = Some("gemini".to_string());

        let resolved = resolve(&settings);
        assert!(resolved.default_harness.is_none());
        assert!(
            resolved
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("settings.default_harness"))
        );
    }

    #[test]
    fn parse_harness_order_preserves_original_positions() {
        let mut settings = settings_with_links(None);
        settings.harness_order = Some(vec!["pi".to_string(), "codex".to_string()]);

        let resolved = resolve(&settings);
        let order = resolved.harness_order.expect("harness order should be set");
        assert_eq!(order.candidates[0].harness, HarnessId::Pi);
        assert_eq!(order.candidates[0].position, 0);
        assert_eq!(order.candidates[1].harness, HarnessId::Codex);
        assert_eq!(order.candidates[1].position, 1);
    }

    #[test]
    fn linked_harnesses_come_from_known_links_only() {
        let settings = settings_with_links(Some(vec![".opencode", ".agents", "foo/bar"]));
        let resolved = resolve(&settings);

        assert!(resolved.linked_harnesses.contains(&HarnessId::OpenCode));
        assert!(!resolved.linked_harnesses.contains(&HarnessId::Codex));
    }

    #[test]
    fn from_config_falls_back_to_default_settings_when_missing() {
        let temp = TempDir::new().expect("temp dir");
        let resolved = ResolvedRoutingSettings::from_config(temp.path());
        assert_eq!(resolved, resolve(&Settings::default()));
    }
}
