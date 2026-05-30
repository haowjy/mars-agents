// qa-validated: harness-order-settings-audit

use std::collections::HashSet;
use std::path::PathBuf;

use crate::harness::host::{
    ExecutableResolver, ExecutableState, PathExecutableResolver,
    native_harness_authenticated as host_native_authed, resolve_binary_path,
};
use crate::harness::registry::{self, HarnessId};

pub const VALID_HARNESSES: &[&str] = &["claude", "codex", "pi", "cursor", "opencode"];

pub fn detect_installed_harnesses() -> HashSet<String> {
    let resolver = PathExecutableResolver;
    registry::all()
        .iter()
        .copied()
        .filter(|id| {
            matches!(
                resolver.resolve(registry::descriptor(*id).binary),
                ExecutableState::Found { .. }
            )
        })
        .map(|id| id.as_str().to_string())
        .collect()
}

pub fn is_valid_harness(name: &str) -> bool {
    registry::is_known(name)
}

pub fn normalize_harness_name(name: &str) -> Option<String> {
    registry::normalize_name(name)
}

pub fn harness_candidates_for_provider(provider: &str) -> Vec<String> {
    registry::provider_candidate_order(provider)
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect()
}

pub fn native_harness_authenticated(harness: &str) -> bool {
    host_native_authed(harness)
}

pub fn resolve_command(command: &str) -> PathBuf {
    let resolver = PathExecutableResolver;
    resolve_binary_path(command, &resolver).unwrap_or_else(|| PathBuf::from(command))
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
                VALID_HARNESSES.join(", ")
            ));
            continue;
        };

        valid_candidates.push(normalized);
    }

    ParsedHarnessOrder {
        valid_candidates,
        warnings,
        failure: None,
    }
}

pub fn parse_harness_id(name: &str) -> Option<HarnessId> {
    registry::parse(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_for_known_provider() {
        let candidates = harness_candidates_for_provider("openai");
        assert_eq!(
            candidates,
            vec!["codex", "claude", "pi", "cursor", "opencode"]
        );
    }

    #[test]
    fn candidates_for_anthropic_native_first_then_default_order() {
        let candidates = harness_candidates_for_provider("anthropic");
        assert_eq!(
            candidates,
            vec!["claude", "codex", "pi", "cursor", "opencode"]
        );
    }

    #[test]
    fn candidates_for_unknown_provider() {
        let candidates = harness_candidates_for_provider("unknown");
        assert_eq!(
            candidates,
            vec!["claude", "codex", "pi", "cursor", "opencode"]
        );
    }

    #[test]
    fn valid_harness_validation_rejects_gemini() {
        assert!(is_valid_harness("claude"));
        assert!(is_valid_harness("OpenCode"));
        assert!(!is_valid_harness("gemini"));
        assert!(!is_valid_harness("unknown"));
    }
}
