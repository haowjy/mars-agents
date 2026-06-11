//! Selective native agent emission when `settings.meridian.agent_copy` is configured.

use crate::compiler::agents::HarnessKind;
use crate::config::AgentCopyConfig;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::registry;

/// Validated harness allowlist for selective native emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCopySpec {
    pub harnesses: Vec<HarnessKind>,
    pub include_fanout: bool,
    pub fanout_agents: Vec<String>,
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
        let Some(kind) = HarnessKind::from_str(trimmed) else {
            diag.warn(
                "agent-copy-invalid-harness",
                format!(
                    "settings.meridian.agent_copy.harnesses: unknown harness '{trimmed}'; \
                     valid harnesses: {}",
                    registry::names().join(", ")
                ),
            );
            continue;
        };
        let target = kind.target_dir();
        if !managed_targets.iter().any(|t| t == target) {
            diag.warn(
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
        fanout_agents: config.fanout_agents.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticCollector;

    #[test]
    fn validate_rejects_unknown_harness_and_missing_target() {
        let config = AgentCopyConfig {
            harnesses: vec!["gemini".to_string(), "claude".to_string()],
            include_fanout: false,
            fanout_agents: Vec::new(),
        };
        let mut diag = DiagnosticCollector::new();
        let spec = build_agent_copy_spec(Some(&config), &[".agents".to_string()], &mut diag);
        assert!(spec.is_none());
        let messages: Vec<_> = diag.drain().into_iter().map(|d| d.message).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("unknown harness 'gemini'")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("not in settings.targets")),
            "{messages:?}"
        );
    }

    #[test]
    fn fanout_agents_propagated_to_spec() {
        let config = AgentCopyConfig {
            harnesses: vec!["claude".to_string()],
            include_fanout: false,
            fanout_agents: vec!["reviewer".to_string(), "investigator".to_string()],
        };
        let mut diag = DiagnosticCollector::new();
        let spec =
            build_agent_copy_spec(Some(&config), &[".claude".to_string()], &mut diag).unwrap();
        assert!(!spec.include_fanout);
        assert_eq!(spec.fanout_agents, vec!["reviewer", "investigator"]);
        assert!(diag.drain().is_empty());
    }

    #[test]
    fn fanout_agents_defaults_to_empty() {
        let config = AgentCopyConfig {
            harnesses: vec!["claude".to_string()],
            include_fanout: false,
            fanout_agents: Vec::new(),
        };
        let mut diag = DiagnosticCollector::new();
        let spec =
            build_agent_copy_spec(Some(&config), &[".claude".to_string()], &mut diag).unwrap();
        assert!(spec.fanout_agents.is_empty());
    }
}
