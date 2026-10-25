use super::*;
use crate::compiler::agents::lower::{self, NativeModel};
use crate::compiler::lossiness::Lossiness;
use crate::frontmatter::Frontmatter;

fn parse(content: &str) -> (AgentProfile, Vec<AgentDiagnostic>) {
    let fm = Frontmatter::parse(content).unwrap();
    let mut diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut diags);
    (profile, diags)
}

fn lower_claude(content: &str) -> (AgentProfile, lower::LoweredOutput, Vec<AgentDiagnostic>) {
    let fm = Frontmatter::parse(content).unwrap();
    let mut diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut diags);
    let out = lower::lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
    (profile, out, diags)
}

fn dropped_invocability(out: &lower::LoweredOutput, field: &str) -> bool {
    out.lossy_fields
        .iter()
        .any(|f| f.field == field && matches!(f.classification, Lossiness::Dropped))
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
fn model_invocable_defaults_true_without_lowering_lossiness() {
    let (p, out, diags) = lower_claude("---\nname: coder\nharness: claude\n---\n# Body");
    assert!(diags.is_empty());
    assert!(p.model_invocable);
    assert!(!dropped_invocability(&out, "model-invocable"));
}

#[test]
fn user_invocable_defaults_true_without_lowering_lossiness() {
    let (p, out, diags) = lower_claude("---\nname: coder\nharness: claude\n---\n# Body");
    assert!(diags.is_empty());
    assert!(p.user_invocable);
    assert!(!dropped_invocability(&out, "user-invocable"));
}

#[test]
fn parses_model_invocable_false() {
    let (p, out, diags) =
        lower_claude("---\nname: coder\nharness: claude\nmodel-invocable: false\n---\n# Body");
    assert!(diags.is_empty());
    assert!(!p.model_invocable);
    assert!(dropped_invocability(&out, "model-invocable"));
}

#[test]
fn parses_user_invocable_false() {
    let (p, out, diags) =
        lower_claude("---\nname: coder\nharness: claude\nuser-invocable: false\n---\n# Body");
    assert!(diags.is_empty());
    assert!(!p.user_invocable);
    assert!(p.model_invocable);
    assert!(dropped_invocability(&out, "user-invocable"));
    assert!(!dropped_invocability(&out, "model-invocable"));
}

#[test]
fn explicit_true_invocability_lowers_without_lossiness() {
    let (p, out, diags) = lower_claude(
        "---\nname: coder\nharness: claude\nmodel-invocable: true\nuser-invocable: true\n---\n# Body",
    );
    assert!(diags.is_empty());
    assert!(p.model_invocable);
    assert!(p.user_invocable);
    assert!(!dropped_invocability(&out, "model-invocable"));
    assert!(!dropped_invocability(&out, "user-invocable"));
}

#[test]
fn snake_case_invocability_keys_parse_and_warn_drop_when_false() {
    let (p, out, diags) = lower_claude(
        "---\nname: coder\nharness: claude\nmodel_invocable: false\nuser_invocable: false\n---\n# Body",
    );
    assert!(diags.is_empty());
    assert!(!p.model_invocable);
    assert!(!p.user_invocable);
    assert!(dropped_invocability(&out, "model-invocable"));
    assert!(dropped_invocability(&out, "user-invocable"));
}

#[test]
fn invalid_model_invocable_produces_diagnostic_and_omits_lossiness() {
    let content = "---\nname: coder\nharness: claude\nmodel-invocable: nope\n---\n# Body";
    let (p, _, diags) = lower_claude(content);
    assert!(p.model_invocable);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "model-invocable")
    );
    let (_, out, _) = lower_claude(content);
    assert!(!dropped_invocability(&out, "model-invocable"));
}

#[test]
fn invalid_user_invocable_produces_diagnostic_and_omits_lossiness() {
    let content = "---\nname: coder\nharness: claude\nuser-invocable: 7\n---\n# Body";
    let (p, _, diags) = lower_claude(content);
    assert!(p.user_invocable);
    assert_eq!(diags.len(), 1);
    assert!(
        matches!(&diags[0], AgentDiagnostic::InvalidFieldValue { field, .. } if field == "user-invocable")
    );
    let (_, out, _) = lower_claude(content);
    assert!(!dropped_invocability(&out, "user-invocable"));
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
fn network_rejects_present_non_string_value() {
    let (_, diags) = parse("---\nnetwork: false\n---\n");
    assert!(diags.iter().any(|diagnostic| {
        matches!(
            diagnostic,
            AgentDiagnostic::InvalidFieldValue { field, .. } if field == "network"
        ) && diagnostic.is_error()
    }));
}

#[test]
fn network_model_policy_rejects_present_non_string_value() {
    let content = "---\nmodel-policies:\n  - match:\n      alias: opus\n    override:\n      network: false\n---\n";
    let (profile, diags) = parse(content);
    assert!(profile.model_policies.is_empty());
    assert!(diags.iter().any(|diagnostic| matches!(
        diagnostic,
        AgentDiagnostic::InvalidFieldValue { field, .. }
            if field == "model-policies[1].override.network"
    )));
}

#[test]
fn parses_harness_claude() {
    let (p, diags) = parse("---\nharness: claude\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessId::Claude));
}

#[test]
fn parses_harness_codex() {
    let (p, diags) = parse("---\nharness: codex\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessId::Codex));
}

#[test]
fn parses_harness_opencode() {
    let (p, diags) = parse("---\nharness: opencode\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessId::OpenCode));
}

#[test]
fn parses_harness_cursor() {
    let (p, diags) = parse("---\nharness: cursor\n---\n");
    assert!(diags.is_empty());
    assert_eq!(p.harness, Some(HarnessId::Cursor));
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
    for s in ["default", "auto", "confirm", "never"] {
        let content = format!("---\napproval: {s}\n---\n");
        let (p, diags) = parse(&content);
        assert!(diags.is_empty(), "unexpected diags for approval={s}");
        assert!(p.approval.is_some());
    }
}

#[test]
fn approval_yolo_parses_with_deprecation_warning() {
    let content = "---\napproval: yolo\n---\n";
    let (p, diags) = parse(content);
    assert_eq!(p.approval, Some(ApprovalMode::Never));
    assert_eq!(diags.len(), 1);
    assert!(!diags[0].is_error());
    assert!(diags[0].message().contains("deprecated"));
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
fn parses_network_all_values() {
    for (value, expected) in [
        ("allow", NetworkPolicy::Allow),
        ("deny", NetworkPolicy::Deny),
        ("allowlist", NetworkPolicy::Allowlist),
    ] {
        let content = format!("---\nnetwork: {value}\n---\n");
        let (profile, diags) = parse(&content);
        assert!(
            diags.is_empty(),
            "unexpected diagnostics for network={value}: {diags:?}"
        );
        assert_eq!(profile.network, Some(expected));
    }

    let (profile, diags) = parse(
        "---\nnetwork_allow: [GitHub.COM., '*.Example.com', '**.all.example', '::ffff:127.0.0.1']\nnetwork_deny: [blocked.example]\n---\n",
    );
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(
        profile
            .network_allow
            .entries()
            .iter()
            .map(NetworkHost::as_str)
            .collect::<Vec<_>>(),
        ["github.com", "*.example.com", "**.all.example", "127.0.0.1"]
    );

    for invalid in [
        "https://example.com",
        "example.com:443",
        "/tmp/socket",
        "*",
        "127.1",
        "256.1.1.1",
        "host.0x7f",
        "*.com",
        "**.com",
    ] {
        let (_, diags) = parse(&format!("---\nnetwork-allow: ['{invalid}']\n---\n"));
        assert!(
            diags.iter().any(AgentDiagnostic::is_error),
            "{invalid}: {diags:?}"
        );
    }
    let (_, unicode_diags) = parse("---\nnetwork-allow: ['münchen.example']\n---\n");
    assert!(
        unicode_diags
            .iter()
            .any(|diagnostic| diagnostic.message().contains("punycode")),
        "{unicode_diags:?}"
    );

    let (_, diags) = parse("---\nnetwork-deny: { replace: [blocked.example] }\n---\n");
    assert!(diags.iter().any(|diagnostic| {
        matches!(
            diagnostic,
            AgentDiagnostic::InvalidFieldValue { field, allowed, .. }
                if field == "network-deny"
                    && allowed.contains("package profiles cannot replace a deny set")
        )
    }));

    let (_, diags) = parse("---\nnetwork_access: deny\ndefault-network-mode: deny\n---\n");
    assert_eq!(
        diags
            .iter()
            .filter(|diagnostic| matches!(diagnostic, AgentDiagnostic::UnknownNetworkField { .. }))
            .count(),
        2,
        "{diags:?}"
    );
    assert!(diags.iter().all(|diagnostic| !diagnostic.is_error()));
}

#[test]
fn duplicate_network_separator_spellings_are_errors() {
    for (field, alias) in [
        ("network-allow", "network_allow"),
        ("network-deny", "network_deny"),
    ] {
        let (_, diags) = parse(&format!(
            "---\n{field}: [a.example]\n{alias}: [b.example]\n---\n"
        ));
        assert!(
            diags.iter().any(|diagnostic| matches!(
                diagnostic,
                AgentDiagnostic::DuplicateField {
                    field: found_field,
                    alias: found_alias,
                } if found_field == field && found_alias == alias
            ) && diagnostic.is_error()),
            "{field}/{alias}: {diags:?}"
        );
    }
}

#[test]
fn invalid_network_string_names_the_field() {
    let (profile, diags) = parse("---\nnetwork: default\n---\n");
    assert_eq!(profile.network, None);
    assert!(diags.iter().any(|diagnostic| {
        matches!(
            diagnostic,
            AgentDiagnostic::InvalidFieldValue { field, .. } if field == "network"
        ) && diagnostic.is_error()
    }));
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
fn harness_override_autocompact_pct_is_passthrough() {
    let content = "---
harness-overrides:
  claude:
    autocompact_pct: 75
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    let claude = p.harness_overrides.entries.get("claude").unwrap();
    assert_eq!(claude["autocompact_pct"], serde_json::json!(75));
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
    let content = "---\nskills: [review, dev-principles]\ntools:\n  Bash: allow\n  Write: allow\n  mcp(server): allow\ndisallowed-tools: [Agent]\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.skills.load, vec!["review", "dev-principles"]);
    assert!(p.skills.available.is_empty());
    assert_eq!(
        p.tools.describe(),
        [
            "allow:bash",
            "allow:write",
            "allow:mcp(server/*)",
            "deny:agent"
        ]
    );
}

#[test]
fn parses_tools_map_allow_and_deny_with_canonical_names() {
    let content =
        "---\ntools:\n  Bash: allow\n  \"Bash(meridian spawn *)\": allow\n  Agent: deny\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(
        p.tools.describe(),
        ["allow:bash", "allow:bash(meridian spawn *)", "deny:agent"]
    );
}

#[test]
fn separator_tool_aliases_canonicalize() {
    let content = "---\ntools:\n  ask_user: allow\n  \"bash(git *)\": deny\ndisallowed-tools: [web_search]\n---\n";
    let (p, diags) = parse(content);

    assert_eq!(
        p.tools.describe(),
        ["allow:ask_user", "deny:bash(git *)", "deny:web_search"]
    );
    assert!(diags.is_empty());
}

#[test]
fn tools_allow_deny_shorthand_is_rejected_not_read_as_a_tool_name() {
    for shorthand in ["allow", "deny"] {
        let (p, diags) = parse(&format!("---\ntools: {shorthand}\n---\n"));
        assert!(
            p.tools.rules.is_empty(),
            "{shorthand} must not become a tool"
        );
        assert!(
            diags.iter().any(|d| matches!(
                d,
                AgentDiagnostic::InvalidFieldValue { field, value, .. }
                    if field == "tools" && value == shorthand
            )),
            "expected an invalid-value diagnostic for `tools: {shorthand}`: {diags:?}"
        );
    }

    let (profile, diags) = parse("---\ntools: { replace: [read] }\n---\n");
    assert!(profile.tools.rules.is_empty());
    assert!(diags.iter().any(|diagnostic| matches!(
        diagnostic,
        AgentDiagnostic::InvalidFieldValue { field, allowed, .. }
            if field == "tools"
                && allowed.contains("package profiles cannot replace a permission set")
    )));

    let (profile, diags) = parse("---\ndisallowed-tools: { replace: [write] }\n---\n");
    assert!(profile.tools.rules.is_empty());
    assert!(diags.iter().any(|diagnostic| matches!(
        diagnostic,
        AgentDiagnostic::InvalidFieldValue { field, allowed, .. }
            if field == "disallowed-tools"
                && allowed.contains("package profiles cannot replace a deny set")
    )));
}

#[test]
fn claude_native_agent_records_ask_rules_as_dropped_not_silently_lost() {
    let (_, out, diags) = lower_claude(
        "---\nname: a\nharness: claude\ntools:\n  read: allow\n  'bash(git push *)': ask\n---\n",
    );
    assert!(diags.is_empty());
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(
        !text.contains("git push"),
        "ask has no frontmatter field: {text}"
    );
    assert!(
        out.lossy_fields
            .iter()
            .any(|f| f.field == "tools (ask)" && matches!(f.classification, Lossiness::Dropped)),
        "{:?}",
        out.lossy_fields
    );
}

#[test]
fn claude_native_agent_lowers_explicit_allowlist_to_native_tools() {
    let (_, out, diags) = lower_claude(
        "---\nname: a\nharness: claude\ntools:\n  '*': deny\n  read: allow\n  grep: allow\n---\n",
    );
    assert!(diags.is_empty(), "{diags:?}");
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(text.contains("tools:") && text.contains("- Read") && text.contains("- Grep"));
    assert!(
        !out.lossy_fields
            .iter()
            .any(|f| f.field.starts_with("tools")),
        "an explicit allowlist is exactly Claude's `tools:`: {:?}",
        out.lossy_fields
    );
}

#[test]
fn claude_native_agent_does_not_turn_map_form_grants_into_an_allowlist() {
    // `tools:` in an agent file restricts the agent to what it lists; a grant does not.
    let (_, out, _) = lower_claude(
        "---\nname: a\nharness: claude\ntools:\n  bash: allow\n  write: allow\n  agent: deny\n---\n",
    );
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(!text.contains("\ntools:"), "{text}");
    assert!(
        text.contains("disallowed-tools:") && text.contains("- Agent"),
        "{text}"
    );
    // Omitting `tools:` inherits every tool, so a map-form grant loses nothing.
    assert!(
        !out.lossy_fields
            .iter()
            .any(|f| f.field.starts_with("tools")),
        "{:?}",
        out.lossy_fields
    );
}

#[test]
fn claude_native_agent_warns_when_a_native_stays_allowed_and_denied() {
    let (_, out, _) = lower_claude(
        "---\nname: a\nharness: claude\ntools:\n  '*': deny\n  read: allow\n  grep: allow\ndisallowed-tools: [grep]\n---\n",
    );
    assert!(
        out.lossy_fields
            .iter()
            .any(|f| f.field == "tools (Grep both allowed and denied)"),
        "{:?}",
        out.lossy_fields
    );
}

#[test]
fn claude_native_agent_cannot_hold_an_allowlist_with_nothing_allowed() {
    // An omitted `tools:` inherits every tool, so deny-all has no frontmatter spelling.
    let (_, out, _) = lower_claude("---\nname: a\nharness: claude\ntools:\n  '*': deny\n---\n");
    assert!(
        out.lossy_fields
            .iter()
            .any(|f| f.field == "tools (allowlist)"
                && matches!(f.classification, Lossiness::Dropped)),
        "{:?}",
        out.lossy_fields
    );
}

#[test]
fn non_claude_native_agents_drop_ask_and_allowlists_with_lossiness() {
    let content = "---\nname: a\ntools:\n  'bash(git push *)': ask\n---\n";
    let fm = Frontmatter::parse(content).unwrap();
    let mut diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut diags);
    for out in [
        lower::lower_to_codex(&profile, fm.body(), &NativeModel::Inherit),
        lower::lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit),
        lower::lower_to_pi(&profile, fm.body(), &NativeModel::Inherit),
    ] {
        assert!(
            out.lossy_fields
                .iter()
                .any(|f| f.field == "tools" && matches!(f.classification, Lossiness::Dropped)),
            "{:?}",
            out.lossy_fields
        );
    }
}

#[test]
fn unknown_pascal_case_tool_names_convert_to_snake_case() {
    let content = "---
tools:
  customtool: allow
  CustomTool: allow
---
";
    let (p, diags) = parse(content);

    assert_eq!(
        p.tools.describe(),
        ["allow:customtool", "allow:custom_tool"]
    );
    assert!(diags.is_empty());
}

#[test]
fn harness_overrides_do_not_replace_tool_policy() {
    let content = "---
tools:
  Bash: allow
  Read: deny
  mcp(plugin:base): allow
disallowed-tools: [Edit]
harness-overrides:
  codex:
    tools: [shell]
    disallowed-tools: [file_write]
    mcp-tools: [plugin:codex]
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    assert_eq!(
        p.tools.describe(),
        vec![
            "allow:bash",
            "deny:read",
            "allow:mcp(plugin:base/*)",
            "deny:edit"
        ]
    );
    assert_eq!(
        p.harness_overrides.entries["codex"]["tools"],
        serde_json::json!(["shell"])
    );
}

#[test]
fn effective_skills_ignore_harness_overrides_passthrough() {
    let content = "---
skills: [base]
harness-overrides:
  codex:
    skills: [codex-only]
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.effective_skills(&HarnessId::Codex).load, vec!["base"]);
    assert_eq!(
        p.harness_overrides.entries["codex"]["skills"],
        serde_json::json!(["codex-only"])
    );
}

#[test]
fn parses_structured_skills_and_override() {
    let content = "---
skills:
  load: [dev-principles]
  available: [planning, spawn]
harness-overrides:
  codex:
    skills:
      load: [codex-principles]
      available: [codex-planning]
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    let codex = p.effective_skills(&HarnessId::Codex);
    assert_eq!(codex.load, vec!["dev-principles"]);
    assert_eq!(codex.available, vec!["planning", "spawn"]);
    assert!(p.harness_overrides.entries.contains_key("codex"));
}

#[test]
fn effective_native_config_uses_matching_harness_passthrough() {
    let content = "---
harness-overrides:
  claude:
    ui.theme: dark
  codex:
    sandbox_workspace_write.network_access: true
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(
        p.effective_native_config(&HarnessId::Codex).unwrap()["sandbox_workspace_write.network_access"],
        serde_json::json!(true)
    );
    assert_eq!(
        p.effective_native_config(&HarnessId::Claude).unwrap()["ui.theme"],
        serde_json::json!("dark")
    );
    assert!(p.effective_native_config(&HarnessId::OpenCode).is_none());
}

// --- 3.1: harness-overrides ---

#[test]
fn harness_overrides_preserve_target_native_passthrough() {
    let content = "---
harness-overrides:
  codex:
    tools: [shell, ask_user, askuser]
    sandbox_workspace_write.network_access: true
    limits:
      max_tokens: 4096
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let codex = p.harness_overrides.entries.get("codex").unwrap();
    assert_eq!(
        codex["tools"],
        serde_json::json!(["shell", "ask_user", "askuser"])
    );
    assert_eq!(
        codex["sandbox_workspace_write.network_access"],
        serde_json::json!(true)
    );
    assert_eq!(codex["limits"], serde_json::json!({"max_tokens": 4096}));
}

#[test]
fn harness_overrides_reject_non_serializable_null_values() {
    let content = "---
harness-overrides:
  codex:
    maybe_null: null
---
";
    let (p, diags) = parse(content);
    assert!(!p.harness_overrides.entries.contains_key("codex"));
    assert!(
        diags.iter().any(|diag| {
            matches!(
                diag,
                AgentDiagnostic::InvalidFieldValue { field, .. }
                    if field == "harness-overrides.codex.maybe_null"
            )
        }),
        "missing nested null diagnostic: {diags:?}"
    );
}

#[test]
fn harness_overrides_preserve_valid_siblings_when_values_are_invalid() {
    let content = "---
harness-overrides:
  codex:
    valid: true
    maybe_null: null
    sequence: [one, null, two]
    mapping:
      kept: 1
      dropped: null
---
";
    let (p, diags) = parse(content);

    let codex = p.harness_overrides.entries.get("codex").unwrap();
    assert_eq!(codex["valid"], serde_json::json!(true));
    assert_eq!(codex["sequence"], serde_json::json!(["one", "two"]));
    assert_eq!(codex["mapping"], serde_json::json!({"kept": 1}));
    assert!(!codex.contains_key("maybe_null"));
    assert!(diags.iter().any(|diag| {
        matches!(
            diag,
            AgentDiagnostic::InvalidFieldValue { field, .. }
                if field == "harness-overrides.codex.maybe_null"
        )
    }));
    assert!(diags.iter().any(|diag| {
        matches!(
            diag,
            AgentDiagnostic::InvalidFieldValue { field, .. }
                if field == "harness-overrides.codex.sequence[1]"
        )
    }));
    assert!(diags.iter().any(|diag| {
        matches!(
            diag,
            AgentDiagnostic::InvalidFieldValue { field, .. }
                if field == "harness-overrides.codex.mapping.dropped"
        )
    }));
}

#[test]
fn harness_overrides_require_mapping_values() {
    let content = "---
harness-overrides:
  codex: [1, 2]
---
";
    let (_p, diags) = parse(content);
    assert!(
        diags.iter().any(|diag| {
            matches!(diag, AgentDiagnostic::InvalidFieldValue { field, .. } if field == "harness-overrides.codex")
        }),
        "missing invalid shape diagnostic: {diags:?}"
    );
}

#[test]
fn harness_overrides_unknown_harness_still_warns_but_preserves_block() {
    let content = "---
harness-overrides:
  future:
    nativeTool: true
---
";
    let (p, diags) = parse(content);
    assert!(p.harness_overrides.entries.contains_key("future"));
    assert!(diags.iter().any(
        |diag| matches!(diag, AgentDiagnostic::UnknownHarnessOverride { value } if value == "future")
    ));
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
    let (p, diags) =
        parse("---\nname: orchestrator\nsubagents:\n  - coder\n  - reviewer\n---\n# Orchestrator");
    assert!(diags.is_empty());
    assert_eq!(p.subagents, vec!["coder", "reviewer"]);
}

#[test]
fn subagents_absent_gives_empty_vec() {
    let (p, diags) = parse("---\nname: solo\n---\n# Solo agent");
    assert!(diags.is_empty());
    assert!(p.subagents.is_empty());
}

#[test]
fn removed_mcp_tools_field_emits_removed_field_diagnostic() {
    let (p, diags) = parse("---\nname: a\ndescription: d\nmcp-tools: [plugin:demo]\n---\n# body");
    assert!(p.tools.rules.is_empty());
    assert!(
        diags.iter().any(|d| matches!(
            d,
            AgentDiagnostic::RemovedField { field } if field == "mcp-tools"
        )),
        "expected RemovedField for mcp-tools, got {diags:?}"
    );
    assert!(diags.iter().any(|d| d.is_error()));
}

#[test]
fn malformed_disallowed_mcp_ref_is_hard_validation_error() {
    let (p, diags) = parse("---\nname: a\ndescription: d\ndisallowed-tools: [mcp()]\n---\n# body");
    assert!(p.tools.rules.is_empty());
    assert!(
        diags.iter().any(|d| matches!(
            d,
            AgentDiagnostic::InvalidFieldValue { field, value, .. }
                if field == "disallowed-tools[0]" && value == "mcp()"
        )),
        "expected invalid tool diagnostic, got {diags:?}"
    );
    assert!(diags.iter().any(|d| d.is_error()));
}
