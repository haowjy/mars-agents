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
    let content = "---\nskills: [review, dev-principles]\ntools: [Bash, Write]\ndisallowed-tools: [Agent]\nmcp-tools: [server]\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.skills.load, vec!["review", "dev-principles"]);
    assert!(p.skills.available.is_empty());
    assert_eq!(p.tools, vec!["bash", "write"]);
    assert!(p.tools_denied.is_empty());
    assert_eq!(p.disallowed_tools, vec!["agent"]);
    assert_eq!(p.mcp_tools, vec!["server"]);
}

#[test]
fn parses_tools_map_allow_and_deny_with_canonical_names() {
    let content =
        "---\ntools:\n  Bash: allow\n  \"Bash(meridian spawn *)\": allow\n  Agent: deny\n---\n";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());
    assert_eq!(p.tools, vec!["bash", "bash(meridian spawn *)"]);
    assert_eq!(p.tools_denied, vec!["agent"]);
}

#[test]
fn separator_tool_aliases_canonicalize() {
    let content = "---\ntools:\n  ask_user: allow\n  \"bash(git *)\": deny\ndisallowed-tools: [web_search]\n---\n";
    let (p, diags) = parse(content);

    assert_eq!(p.tools, vec!["ask_user"]);
    assert_eq!(p.tools_denied, vec!["bash(git *)"]);
    assert_eq!(p.disallowed_tools, vec!["web_search"]);
    assert!(diags.is_empty());
}

#[test]
fn unknown_pascal_case_tool_names_convert_to_snake_case() {
    let content = "---
tools: [customtool, CustomTool]
---
";
    let (p, diags) = parse(content);

    assert_eq!(p.tools, vec!["customtool", "custom_tool"]);
    assert!(diags.is_empty());
}

#[test]
fn harness_overrides_do_not_replace_tool_policy() {
    let content = "---
tools:
  Bash: allow
  Read: deny
disallowed-tools: [Edit]
mcp-tools: [plugin:base]
harness-overrides:
  codex:
    tools: [shell]
    disallowed-tools: [file_write]
    mcp-tools: [plugin:codex]
---
";
    let (p, diags) = parse(content);
    assert!(diags.is_empty());

    let codex_policy = p.effective_tool_policy(&HarnessKind::Codex);
    assert_eq!(codex_policy.allowed, vec!["bash"]);
    assert_eq!(codex_policy.disallowed, vec!["read", "edit"]);
    assert_eq!(codex_policy.mcp, vec!["plugin:base"]);
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
    assert_eq!(p.effective_skills(&HarnessKind::Codex).load, vec!["base"]);
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

    let codex = p.effective_skills(&HarnessKind::Codex);
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
        p.effective_native_config(&HarnessKind::Codex).unwrap()["sandbox_workspace_write.network_access"],
        serde_json::json!(true)
    );
    assert_eq!(
        p.effective_native_config(&HarnessKind::Claude).unwrap()["ui.theme"],
        serde_json::json!("dark")
    );
    assert!(p.effective_native_config(&HarnessKind::OpenCode).is_none());
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
