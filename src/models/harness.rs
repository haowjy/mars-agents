// qa-validated: harness-order-settings-audit

use crate::harness::host::{
    ExecutableResolver, ExecutableState, PathExecutableResolver,
    native_harness_authenticated as host_native_authed,
};
use crate::harness::registry;
use std::collections::HashSet;

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
pub struct ParsedHarnessOrder {
    pub valid_candidates: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn parse_settings_harness_order(order: &[String]) -> ParsedHarnessOrder {
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
    }
}
