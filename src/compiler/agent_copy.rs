//! Selective native agent emission when `settings.meridian.agent_copy` is configured.

use crate::config::AgentCopyConfig;
use crate::diagnostic::DiagnosticCollector;
use mars_schema::HarnessId;

/// Validated harness allowlist for selective native emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCopySpec {
    pub harnesses: Vec<HarnessId>,
    pub include_fanout: bool,
}

/// Validate `agent_copy.harnesses` and build a spec for the compiler.
pub fn build_agent_copy_spec(
    config: Option<&AgentCopyConfig>,
    managed_targets: &[String],
    diag: &mut DiagnosticCollector,
) -> Option<AgentCopySpec> {
    let config = config?;
    if config.harnesses.is_empty() {
        return None;
    }

    let mut harnesses = Vec::new();
    for name in &config.harnesses {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(kind) = mars_schema::harness::parse(trimmed) else {
            diag.warn_advisory(
                "agent-copy-invalid-harness",
                format!(
                    "settings.meridian.agent_copy.harnesses: unknown harness '{trimmed}'; \
                     valid harnesses: {}",
                    mars_schema::harness::names()
                ),
            );
            continue;
        };
        let target = crate::harness::registry::descriptor(kind).default_target;
        if !managed_targets.iter().any(|t| t == target) {
            diag.warn_advisory(
                "agent-copy-harness-not-in-targets",
                format!(
                    "settings.meridian.agent_copy.harnesses: harness '{trimmed}' maps to target \
                     `{target}` which is not in settings.targets; add `{target}` to \
                     settings.targets to emit native agents there"
                ),
            );
            continue;
        }
        if !harnesses.contains(&kind) {
            harnesses.push(kind);
        }
    }

    if harnesses.is_empty() {
        return None;
    }

    Some(AgentCopySpec {
        harnesses,
        include_fanout: config.include_fanout,
    })
}
