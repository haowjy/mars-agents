//! Selective native agent emission when `settings.agent_copy` is configured.

use indexmap::IndexMap;

use crate::compiler::agents::{AgentProfile, HarnessKind};
use crate::config::{AgentCopyConfig, ModelPolicyMatchType, ModelPolicyRule};
use crate::diagnostic::DiagnosticCollector;
use crate::harness::registry;
use crate::models::{ModelAlias, ModelSpec};

/// Validated harness allowlist for selective native emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCopySpec {
    pub harnesses: Vec<HarnessKind>,
    pub include_fanout: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QualifiedEmission {
    DefaultModel,
    PolicyModel(String),
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
                    "settings.agent_copy.harnesses: unknown harness '{trimmed}'; \
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
                    "settings.agent_copy.harnesses: harness '{trimmed}' maps to target \
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

pub fn agent_qualifies_for_harness(
    profile: &AgentProfile,
    target_harness: &HarnessKind,
    model_aliases: &IndexMap<String, ModelAlias>,
    include_fanout: bool,
) -> Option<QualifiedEmission> {
    if profile.harness.as_ref() == Some(target_harness) {
        return Some(QualifiedEmission::DefaultModel);
    }

    if let Some(ref model_token) = profile.model
        && model_resolves_to_harness(model_token, target_harness, model_aliases)
    {
        return Some(QualifiedEmission::DefaultModel);
    }

    if !include_fanout {
        return None;
    }

    for policy in &profile.model_policies {
        if let Some(emission) = policy_qualifies(policy, target_harness, model_aliases) {
            return Some(emission);
        }
    }

    None
}

fn policy_qualifies(
    policy: &ModelPolicyRule,
    target_harness: &HarnessKind,
    model_aliases: &IndexMap<String, ModelAlias>,
) -> Option<QualifiedEmission> {
    if let Some(override_harness) = policy_override_harness(policy)
        && override_harness != *target_harness
    {
        return None;
    }

    match policy.match_type {
        ModelPolicyMatchType::Alias => {
            if model_resolves_to_harness(&policy.match_value, target_harness, model_aliases) {
                return Some(QualifiedEmission::PolicyModel(policy.match_value.clone()));
            }
        }
        ModelPolicyMatchType::Model => {
            for (alias_name, alias) in model_aliases {
                if alias.pinned_model_id() == Some(policy.match_value.as_str())
                    && alias_resolves_to_harness(alias, target_harness)
                {
                    return Some(QualifiedEmission::PolicyModel(alias_name.clone()));
                }
            }
        }
        ModelPolicyMatchType::ModelGlob => {
            for (alias_name, alias) in model_aliases {
                let Some(model_id) = alias.pinned_model_id() else {
                    continue;
                };
                if crate::models::glob_match(&policy.match_value, model_id)
                    && alias_resolves_to_harness(alias, target_harness)
                {
                    return Some(QualifiedEmission::PolicyModel(alias_name.clone()));
                }
            }
        }
    }

    None
}

fn alias_resolves_to_harness(alias: &ModelAlias, target_harness: &HarnessKind) -> bool {
    if let Some(ref harness_name) = alias.harness {
        return HarnessKind::from_str(harness_name).as_ref() == Some(target_harness);
    }

    let provider = match &alias.spec {
        ModelSpec::Pinned { provider, .. } | ModelSpec::PinnedWithMatch { provider, .. } => {
            provider.as_deref()
        }
        ModelSpec::AutoResolve { provider, .. } => provider.as_deref(),
    };

    if let Some(provider) = provider
        && let Some(native) = registry::native_harness_for_provider(provider)
    {
        return HarnessKind::from_harness_id(native) == *target_harness;
    }

    false
}

fn policy_override_harness(policy: &ModelPolicyRule) -> Option<HarnessKind> {
    policy
        .overrides
        .get(serde_yaml::Value::String("harness".to_string()))
        .and_then(|value| value.as_str())
        .and_then(HarnessKind::from_str)
}

pub fn model_resolves_to_harness(
    model_token: &str,
    target_harness: &HarnessKind,
    aliases: &IndexMap<String, ModelAlias>,
) -> bool {
    let Some(alias) = aliases.get(model_token) else {
        return false;
    };

    if let Some(ref harness_name) = alias.harness {
        return HarnessKind::from_str(harness_name).as_ref() == Some(target_harness);
    }

    let provider = match &alias.spec {
        ModelSpec::Pinned { provider, .. } | ModelSpec::PinnedWithMatch { provider, .. } => {
            provider.as_deref()
        }
        ModelSpec::AutoResolve { provider, .. } => provider.as_deref(),
    };

    if let Some(provider) = provider
        && let Some(native) = registry::native_harness_for_provider(provider)
    {
        return HarnessKind::from_harness_id(native) == *target_harness;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::AgentProfile;
    use crate::config::ModelPolicyMatchType;
    use crate::diagnostic::DiagnosticCollector;
    use crate::frontmatter::SkillsSpec;
    use crate::models::{ModelAlias, ModelSpec};

    fn empty_profile() -> AgentProfile {
        AgentProfile {
            name: None,
            description: None,
            harness: None,
            model: None,
            mode: None,
            model_invocable: true,
            approval: None,
            sandbox: None,
            effort: None,
            autocompact: None,
            autocompact_pct: None,
            skills: SkillsSpec::default(),
            subagents: Vec::new(),
            tools: Vec::new(),
            tools_denied: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_tools: Vec::new(),
            harness_overrides: Default::default(),
            model_policies: Vec::new(),
            fanout: Vec::new(),
        }
    }

    fn anthropic_alias() -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "claude-opus-4-6".to_string(),
                provider: Some("anthropic".to_string()),
            },
        }
    }

    #[test]
    fn explicit_profile_harness_qualifies() {
        let mut profile = empty_profile();
        profile.harness = Some(HarnessKind::Claude);
        assert!(
            agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &IndexMap::new(), false,)
                .is_some()
        );
    }

    #[test]
    fn model_alias_provider_maps_to_claude() {
        let mut profile = empty_profile();
        profile.model = Some("opus".to_string());
        let mut aliases = IndexMap::new();
        aliases.insert("opus".to_string(), anthropic_alias());
        assert!(
            agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &aliases, false).is_some()
        );
    }

    #[test]
    fn unknown_model_alias_does_not_qualify() {
        let mut profile = empty_profile();
        profile.model = Some("missing".to_string());
        assert!(
            agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &IndexMap::new(), false,)
                .is_none()
        );
    }

    #[test]
    fn fanout_policy_qualifies_with_match_value() {
        let mut profile = empty_profile();
        profile.model_policies.push(ModelPolicyRule {
            match_type: ModelPolicyMatchType::Alias,
            match_value: "sonnet".to_string(),
            no_fallback: false,
            overrides: serde_yaml::Mapping::new(),
        });
        let mut aliases = IndexMap::new();
        aliases.insert(
            "sonnet".to_string(),
            ModelAlias {
                harness: None,
                description: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::Pinned {
                    model: "claude-sonnet-4-6".to_string(),
                    provider: Some("anthropic".to_string()),
                },
            },
        );
        let emission = agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &aliases, true)
            .expect("policy should qualify");
        assert!(matches!(
            emission,
            QualifiedEmission::PolicyModel(ref m) if m == "sonnet"
        ));
    }

    #[test]
    fn model_policy_qualifies_by_pinned_model_id() {
        let mut profile = empty_profile();
        profile.model_policies.push(ModelPolicyRule {
            match_type: ModelPolicyMatchType::Model,
            match_value: "claude-sonnet-4-6".to_string(),
            no_fallback: false,
            overrides: serde_yaml::Mapping::new(),
        });
        let mut aliases = IndexMap::new();
        aliases.insert(
            "sonnet".to_string(),
            ModelAlias {
                harness: None,
                description: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::Pinned {
                    model: "claude-sonnet-4-6".to_string(),
                    provider: Some("anthropic".to_string()),
                },
            },
        );
        let emission = agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &aliases, true)
            .expect("model policy should qualify");
        assert!(matches!(
            emission,
            QualifiedEmission::PolicyModel(ref m) if m == "sonnet"
        ));
    }

    #[test]
    fn model_glob_policy_qualifies_by_pinned_model_id() {
        let mut profile = empty_profile();
        profile.model_policies.push(ModelPolicyRule {
            match_type: ModelPolicyMatchType::ModelGlob,
            match_value: "claude-sonnet-*".to_string(),
            no_fallback: false,
            overrides: serde_yaml::Mapping::new(),
        });
        let mut aliases = IndexMap::new();
        aliases.insert(
            "sonnet".to_string(),
            ModelAlias {
                harness: None,
                description: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::Pinned {
                    model: "claude-sonnet-4-6".to_string(),
                    provider: Some("anthropic".to_string()),
                },
            },
        );
        let emission = agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &aliases, true)
            .expect("model-glob policy should qualify");
        assert!(matches!(
            emission,
            QualifiedEmission::PolicyModel(ref m) if m == "sonnet"
        ));
    }

    #[test]
    fn policy_override_harness_vetoes_mismatch() {
        let mut profile = empty_profile();
        let mut overrides = serde_yaml::Mapping::new();
        overrides.insert(
            serde_yaml::Value::String("harness".to_string()),
            serde_yaml::Value::String("codex".to_string()),
        );
        profile.model_policies.push(ModelPolicyRule {
            match_type: ModelPolicyMatchType::Alias,
            match_value: "gpt".to_string(),
            no_fallback: false,
            overrides,
        });
        let mut aliases = IndexMap::new();
        aliases.insert(
            "gpt".to_string(),
            ModelAlias {
                harness: None,
                description: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::Pinned {
                    model: "gpt-5".to_string(),
                    provider: Some("openai".to_string()),
                },
            },
        );
        assert!(
            agent_qualifies_for_harness(&profile, &HarnessKind::Claude, &aliases, true).is_none()
        );
    }

    #[test]
    fn validate_rejects_unknown_harness_and_missing_target() {
        let config = AgentCopyConfig {
            harnesses: vec!["gemini".to_string(), "claude".to_string()],
            include_fanout: false,
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
}
