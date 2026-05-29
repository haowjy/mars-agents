use super::*;
use crate::config::Config;
use crate::frontmatter::Frontmatter;

fn parse(content: &str) -> (AgentProfile, Vec<AgentDiagnostic>) {
    let fm = Frontmatter::parse(content).unwrap();
    let mut diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut diags);
    (profile, diags)
}

// --- 3.1: Basic field parsing ---

#[test]
fn parses_name_and_description() {
    let (p, diags) = parse("---\nname: coder\ndescription: Code agent\n---\n# Body");
    assert!(diags.is_empty());
    assert_eq!(p.name.as_deref(), Some("coder"));
    assert_eq!(p.description.as_deref(), Some("Code agent"));
}

#[test]
fn parses_mode_primary() {
    let (p, diags) = parse("---\nmode: primary\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.mode, Some(AgentMode::Primary));
}

#[test]
fn parses_mode_subagent() {
    let (p, diags) = parse("---\nmode: subagent\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.mode, Some(AgentMode::Subagent));
}

#[test]
fn model_invocable_defaults_true() {
    let (p, diags) = parse("---\nmode: subagent\n---\n");
    assert!(diags.is_empty());
    assert!(p.model_invocable);
}

#[test]
fn parses_model_invocable_false() {
    let (p, diags) = parse("---\nmodel-invocable: false\n---\n");
    assert!(diags.is_empty());
    assert!(!p.model_invocable);
}

#[test]
fn invalid_model_invocable_produces_diagnostic() {
    let (p, diags) = parse("---\nmodel-invocable: nope\n---\n");
    assert!(p.model_invocable);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "model-invocable")
    );
}

#[test]
fn invalid_mode_produces_diagnostic() {
    let (p, diags) = parse("---\nmode: invalid\n---\n");
    assert_eq!(p.mode, None);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "mode")
    );
}

#[test]
fn parses_harness_claude() {
    let (p, diags) = parse("---\nharness: claude\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessKind::Claude));
}

#[test]
fn parses_harness_codex() {
    let (p, diags) = parse("---\nharness: codex\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessKind::Codex));
}

#[test]
fn parses_harness_opencode() {
    let (p, diags) = parse("---\nharness: opencode\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessKind::OpenCode));
}

#[test]
fn parses_harness_cursor() {
    let (p, diags) = parse("---\nharness: cursor\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessKind::Cursor));
}

#[test]
fn unknown_harness_produces_diagnostic() {
    let (p, diags) = parse("---\nharness: unknown\n---\n");
    assert_eq!(p.harness, None);
    assert_eq!(diags.len(), 1);
    assert!(matches!(&diags[0], AgentDiagnostic::UnknownHarness { value } if value == "unknown"));
}

#[test]
fn parses_effort_all_values() {
    for (s, expected) in [
        ("low", EffortLevel::Low),
        ("medium", EffortLevel::Medium),
        ("high", EffortLevel::High),
        ("xhigh", EffortLevel::XHigh),
    ] {
        let content = format!("---\neffort: {s}\n---\n");
        let (p, diags) = parse(&content);
        assert!(
            diags.is_empty(),
            "unexpected diags for effort={s}: {diags:?}"
        );
        assert_eq!(p.effort, Some(expected));
    }
}

#[test]
fn parses_effort_none_sentinel() {
    // "none" is a valid value meaning "no effort level" — same as omitting the field.
    let (p, diags) = parse("---\neffort: none\n---\n");
    assert!(diags.is_empty(), "unexpected diags: {diags:?}");
    assert_eq!(p.effort, None);
}

#[test]
fn parses_approval_all_values() {
    for s in ["default", "auto", "confirm", "yolo"] {
        let content = format!("---\napproval: {s}\n---\n");
        let (p, diags) = parse(&content);
        assert!(diags.is_empty(), "unexpected diags for approval={s}");
        assert!(p.approval.is_some());
    }
}

#[test]
fn parses_sandbox_all_values() {
    for s in [
        "default",
        "read-only",
        "workspace-write",
        "danger-full-access",
    ] {
        let content = format!("---\nsandbox: {s}\n---\n");
        let (p, diags) = parse(&content);
        assert!(diags.is_empty(), "unexpected diags for sandbox={s}");
        assert!(p.sandbox.is_some());
    }
}

#[test]
fn parses_autocompact() {
    let (p, diags) = parse("---\nautocompact: 50\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.autocompact, Some(50));
}

#[test]
fn parses_autocompact_pct() {
    let (p, diags) = parse("---\nautocompact_pct: 80\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.autocompact_pct, Some(80));
}

#[test]
fn autocompact_pct_out_of_range() {
    let (p, diags) = parse("---\nautocompact_pct: 101\n---\n");
    assert_eq!(p.autocompact_pct, None);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
    );
}

#[test]
fn autocompact_pct_zero_out_of_range() {
    let (p, diags) = parse("---\nautocompact_pct: 0\n---\n");
    assert_eq!(p.autocompact_pct, None);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
    );
}

#[test]
fn autocompact_pct_in_override() {
    let content = "---\nharness-overrides:\n  claude:\n    autocompact_pct: 75\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    let claude = p.harness_overrides.claude.as_ref().unwrap();
    assert_eq!(claude.autocompact_pct, Some(75));
}

#[test]
fn autocompact_string_produces_diagnostic() {
    let (p, diags) = parse("---\nautocompact: \"50\"\n---\n");
    assert_eq!(p.autocompact, None);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact")
    );
}

#[test]
fn autocompact_pct_string_produces_diagnostic() {
    let (p, diags) = parse("---\nautocompact_pct: \"80\"\n---\n");
    assert_eq!(p.autocompact_pct, None);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "autocompact_pct")
    );
}

#[test]
fn parses_skills_tools_disallowed_mcp() {
    let content = "---\nskills: [review, dev-principles]\ntools: [Bash, Write]\ndisallowed-tools: [Agent]\nmcp-tools: [server]\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.skills, vec!["review", "dev-principles"]);
    assert_eq!(p.tools, vec!["Bash", "Write"]);
    assert!(p.tools_denied.is_empty());
    assert_eq!(p.disallowed_tools, vec!["Agent"]);
    assert_eq!(p.mcp_tools, vec!["server"]);
}

#[test]
fn parses_tools_map_allow_and_deny_with_name_normalization() {
    let content =
        "---\ntools:\n  bash: allow\n  \"bash(meridian spawn *)\": allow\n  agent: deny\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.tools, vec!["Bash", "Bash(meridian spawn *)"]);
    assert_eq!(p.tools_denied, vec!["Agent"]);
}

#[test]
fn effective_tool_policy_uses_harness_override_replacements() {
    let content = "---\ntools:\n  bash: allow\n  read: deny\ndisallowed-tools: [Edit]\nmcp-tools: [plugin:base]\nharness-overrides:\n  codex:\n    tools:\n      \"bash(meridian spawn *)\": allow\n      agent: deny\n    disallowed-tools: [Write]\n    mcp-tools: [plugin:codex]\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    let codex_policy = p.effective_tool_policy(&HarnessKind::Codex);
    assert_eq!(codex_policy.allowed, vec!["Bash(meridian spawn *)"]);
    assert_eq!(codex_policy.disallowed, vec!["Agent", "Write"]);
    assert_eq!(codex_policy.mcp, vec!["plugin:codex"]);

    let claude_policy = p.effective_tool_policy(&HarnessKind::Claude);
    assert_eq!(claude_policy.allowed, vec!["Bash"]);
    assert_eq!(claude_policy.disallowed, vec!["Read", "Edit"]);
    assert_eq!(claude_policy.mcp, vec!["plugin:base"]);
}

#[test]
fn effective_skills_use_harness_override_replacement() {
    let content =
        "---\nskills: [base]\nharness-overrides:\n  codex:\n    skills: [codex-only]\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    assert_eq!(
        p.effective_skills(&HarnessKind::Codex),
        &vec!["codex-only".to_string()]
    );
    assert_eq!(
        p.effective_skills(&HarnessKind::Claude),
        &vec!["base".to_string()]
    );
}

#[test]
fn effective_native_config_uses_matching_harness_override() {
    let content = "---\nharness-overrides:\n  claude:\n    native-config:\n      ui.theme: dark\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    assert_eq!(
        p.effective_native_config(&HarnessKind::Codex)
            .expect("codex native config"),
        &serde_json::Map::from_iter([(
            "sandbox_workspace_write.network_access".to_string(),
            serde_json::json!(true)
        )])
    );
    assert_eq!(
        p.effective_native_config(&HarnessKind::Claude)
            .expect("claude native config"),
        &serde_json::Map::from_iter([("ui.theme".to_string(), serde_json::json!("dark"))])
    );
    assert!(p.effective_native_config(&HarnessKind::OpenCode).is_none());
}

// --- 3.1: model-policies ---

#[test]
fn model_policies_are_parsed_as_raw_entries() {
    let content = "---\nmodel-policies:\n  - match:\n      model: gpt-5.5\n    override:\n      harness: codex\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.model_policies.len(), 1);
    assert_eq!(p.model_policies[0].match_type, ModelPolicyMatchType::Model);
    assert_eq!(p.model_policies[0].match_value, "gpt-5.5");
    assert!(p.model_policies[0].overrides.contains_key("harness"));
}

#[test]
fn model_policy_empty_override_is_valid_for_fallback_candidate() {
    let content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n    override: {}\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.model_policies.len(), 1);
    assert!(p.model_policies[0].overrides.is_empty());
}

#[test]
fn model_policy_empty_override_is_valid_for_no_fallback_rule() {
    let content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n    no-fallback: true\n    override: {}\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.model_policies.len(), 1);
    assert!(p.model_policies[0].no_fallback);
    assert!(p.model_policies[0].overrides.is_empty());
}

#[test]
fn model_policy_missing_override_is_valid() {
    let content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.model_policies.len(), 1);
    assert!(p.model_policies[0].overrides.is_empty());
}

#[test]
fn malformed_model_policy_produces_diagnostic() {
    let content = "---\nmodel-policies:\n  - match:\n      model: gpt-5.5\n      alias: gpt55\n    override:\n      harness: codex\n---\n";
    let (p, diags) = parse(content);
    assert!(p.model_policies.is_empty());
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "model-policies[1].match")
    );
}

#[test]
fn model_policy_rule_type_is_shared_across_profile_overlay_and_settings() {
    let profile_content = "---\nmodel-policies:\n  - match:\n      alias: gpt55\n    override:\n      harness: codex\n---\n";
    let (profile, diags) = parse(profile_content);
    assert!(diags.is_empty());

    let config: Config = toml::from_str(
        r#"
[agents.reviewer]

[[agents.reviewer.model-policies]]
match = { alias = "gpt55" }
override = { harness = "codex" }

[settings]

[[settings.model-policies]]
match = { alias = "gpt55" }
override = { harness = "codex" }
"#,
    )
    .unwrap();

    assert_eq!(profile.model_policies.len(), 1);
    assert_eq!(config.agents["reviewer"].model_policies.len(), 1);
    assert_eq!(config.settings.model_policies.len(), 1);
    assert_eq!(
        profile.model_policies[0],
        config.agents["reviewer"].model_policies[0]
    );
    assert_eq!(profile.model_policies[0], config.settings.model_policies[0]);
}

// --- 3.1: fanout ---

#[test]
fn fanout_entries_are_parsed_as_raw() {
    let content = "---\nfanout:\n  - alias: opus\n  - model: gpt-5.5\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.fanout.len(), 2);
}

// --- 3.1: harness-overrides ---

#[test]
fn harness_overrides_parsed_for_claude_and_codex() {
    let content = "---\nharness-overrides:\n  claude:\n    approval: auto\n  codex:\n    sandbox: workspace-write\n    effort: high\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    let claude = p.harness_overrides.claude.as_ref().unwrap();
    assert_eq!(claude.approval, Some(ApprovalMode::Auto));
    let codex = p.harness_overrides.codex.as_ref().unwrap();
    assert_eq!(codex.sandbox, Some(SandboxMode::WorkspaceWrite));
    assert_eq!(codex.effort, Some(EffortLevel::High));
}

#[test]
fn harness_override_native_config_parses_shape_only() {
    let content = "---\nharness-overrides:\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n      limits:\n        max_tokens: 4096\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let codex = p.harness_overrides.codex.as_ref().unwrap();
    let native_config = codex.native_config.as_ref().unwrap();
    assert_eq!(
        native_config["sandbox_workspace_write.network_access"],
        serde_json::json!(true)
    );
    assert_eq!(
        native_config["limits"],
        serde_json::json!({"max_tokens": 4096})
    );
}

#[test]
fn harness_override_native_config_accepts_arrays_and_rejects_null_values() {
    let valid_content = "---\nharness-overrides:\n  codex:\n    native-config:\n      allowlist: [Bash, Read]\n      nested:\n        values: [1, 2]\n---\n";
    let (valid_profile, valid_diags) = parse(valid_content);
    assert!(
        valid_diags.is_empty(),
        "unexpected diagnostics: {valid_diags:?}"
    );
    let valid_native_config = valid_profile
        .harness_overrides
        .codex
        .as_ref()
        .unwrap()
        .native_config
        .as_ref()
        .unwrap();
    assert_eq!(
        valid_native_config["allowlist"],
        serde_json::json!(["Bash", "Read"])
    );
    assert_eq!(
        valid_native_config["nested"],
        serde_json::json!({"values": [1, 2]})
    );

    let null_content =
        "---\nharness-overrides:\n  codex:\n    native-config:\n      maybe_null: null\n---\n";
    let (null_profile, null_diags) = parse(null_content);
    let codex = null_profile.harness_overrides.codex.as_ref().unwrap();
    assert!(
        codex.native_config.is_none(),
        "native-config with a null value should be rejected"
    );
    assert!(
        null_diags.iter().any(|diag| {
            matches!(
                diag,
                AgentDiagnostic::InvalidFieldValue { field, .. }
                    if field == "harness-overrides.codex.native-config.maybe_null"
            )
        }),
        "missing nested null diagnostic: {null_diags:?}"
    );
}

#[test]
fn harness_override_native_config_invalid_shape_produces_diagnostic() {
    let content = "---\nharness-overrides:\n  codex:\n    native-config: [1, 2]\n---\n";
    let (p, diags) = parse(content);
    let codex = p.harness_overrides.codex.as_ref().unwrap();
    assert!(codex.native_config.is_none());
    assert!(
            diags.iter().any(|diag| {
                matches!(diag, AgentDiagnostic::InvalidFieldValue { field, .. } if field == "harness-overrides.codex.native-config")
            }),
            "missing native-config invalid shape diagnostic: {diags:?}"
        );
}

#[test]
fn harness_override_native_config_portable_key_collision_warns() {
    let content =
        "---\nharness-overrides:\n  codex:\n    native-config:\n      sandbox: true\n---\n";
    let (_p, diags) = parse(content);
    assert!(
            diags.iter().any(|diag| {
                matches!(diag, AgentDiagnostic::NativeConfigPortableKeyCollision { key, .. } if key == "sandbox")
            }),
            "expected portable key collision warning: {diags:?}"
        );
}

#[test]
fn harness_override_with_non_overridable_field_produces_diagnostic() {
    let content = "---\nharness-overrides:\n  claude:\n    name: bad\n---\n";
    let (_p, diags) = parse(content);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::NonOverridableFieldInOverride { field, .. } if field == "name")
    );
}

// --- 3.1: legacy models field ---

#[test]
fn legacy_models_field_produces_deprecation_warning() {
    let content = "---\nmodels:\n  opus:\n    effort: high\n---\n";
    let (_p, diags) = parse(content);
    assert_eq!(diags.len(), 1);
    assert!(matches!(&diags[0], AgentDiagnostic::LegacyModelsField));
}

// --- Empty agent ---

#[test]
fn empty_agent_has_no_diagnostics() {
    let (p, diags) = parse("# Minimal agent\nno frontmatter");
    assert!(diags.is_empty());
    assert!(p.name.is_none());
    assert!(p.harness.is_none());
}

#[test]
fn agent_without_harness_is_universal() {
    let (p, _) = parse("---\nname: planner\nmodel: gpt55\n---\n# Planner");
    assert!(p.harness.is_none());
}

// --- subagents field ---

#[test]
fn subagents_list_parses() {
    let (p, diags) = parse(
        "---\nname: orchestrator\nsubagents:\n  - coder\n  - reviewer\n---\n# Orchestrator",
    );
    assert!(diags.is_empty());
    assert_eq!(p.subagents, vec!["coder", "reviewer"]);
}

#[test]
fn subagents_absent_gives_empty_vec() {
    let (p, diags) = parse("---\nname: solo\n---\n# Solo agent");
    assert!(diags.is_empty());
    assert!(p.subagents.is_empty());
}
