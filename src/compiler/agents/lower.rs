//! Public entry points and tests for per-harness agent lowering.
//!
//! The implementation lives in `lower_policy.rs` so the declarative policy table
//! and shared lowering pipeline stay separate from the regression tests.

#[path = "lower_policy.rs"]
mod lower_policy;

pub use lower_policy::{NativeModel, lower_for_harness_with_model};

#[cfg(test)]
pub type LoweredOutput = crate::compiler::lossiness::LoweredOutput;

#[cfg(test)]
pub use lower_policy::{
    lower_to_claude, lower_to_codex, lower_to_cursor_with_model, lower_to_opencode, lower_to_pi,
};

#[cfg(test)]
use crate::compiler::lossiness::Lossiness;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::{
        AgentDiagnostic, AgentProfile, HarnessKind, parse_agent_content,
    };
    use crate::frontmatter::Frontmatter;

    fn profile_from(content: &str) -> (AgentProfile, Frontmatter, Vec<AgentDiagnostic>) {
        let mut diags = Vec::new();
        let (profile, fm) = parse_agent_content(content, &mut diags).unwrap();
        (profile, fm, diags)
    }

    // --- 3.3: Claude lowering ---

    #[test]
    fn claude_lowering_preserves_name_description_model_skills_tools_body() {
        let content = "---\nname: coder\ndescription: Code impl agent\nmodel: gpt55\nharness: claude\nskills: [dev-principles]\ntools: [Bash, Write]\n---\n# Coder\nYou write code.";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body();
        let out = lower_to_claude(&profile, &fm, body, &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: coder"), "name missing: {text}");
        assert!(
            text.contains("description: Code impl agent"),
            "desc missing"
        );
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("skills"), "skills missing");
        assert!(text.contains("tools"), "tools missing");
        assert!(text.contains("# Coder"), "body missing");
    }

    #[test]
    fn claude_lowering_projects_canonical_tool_names() {
        let content = "---\nname: coder\nharness: claude\ntools: [Bash, AskUser]\ndisallowed-tools: [Bash(git reset *)]\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(text.contains("- Bash"), "Bash not projected: {text}");
        assert!(text.contains("- AskUser"), "AskUser not projected: {text}");
        assert!(
            text.contains("- Bash(git reset *)"),
            "scoped Bash not projected while preserving payload: {text}"
        );
    }

    #[test]
    fn claude_lowering_projects_unknown_custom_tools_and_preserves_mcp_wire_identifiers() {
        let content = "---\nname: coder\nharness: claude\ntools: [my_custom_tool, mcp__server__Tool]\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            text.contains("- MyCustomTool"),
            "unknown custom tool should be convention-projected: {text}"
        );
        assert!(
            text.contains("- mcp__server__Tool"),
            "mcp__ wire identifier should be preserved verbatim: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "tools"
                && field.target == "claude"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "unknown tool projected via harness naming convention"
                    }
                )
        }));
    }

    #[test]
    fn codex_and_cursor_agent_lowering_drops_tools_without_native_projection() {
        let content = "---\nname: coder\nharness: codex\ntools: [my_custom_tool, mcp__server__Tool]\n---\n# Body";
        let (profile, fm, _) = profile_from(content);

        let codex = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let codex_text = String::from_utf8(codex.bytes).unwrap();
        assert!(
            !codex_text.contains("my_custom_tool") && !codex_text.contains("mcp__server__Tool"),
            "codex native agent artifacts drop tools entirely: {codex_text}"
        );
        assert!(codex.lossy_fields.iter().any(|field| {
            field.field == "tools" && matches!(field.classification, Lossiness::Dropped)
        }));

        let cursor = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        let cursor_text = String::from_utf8(cursor.bytes).unwrap();
        assert!(
            !cursor_text.contains("MyCustomTool")
                && !cursor_text.contains("my_custom_tool")
                && !cursor_text.contains("mcp__server__Tool"),
            "cursor native agent artifacts drop tools entirely: {cursor_text}"
        );
        assert!(cursor.lossy_fields.iter().any(|field| {
            field.field == "tools" && matches!(field.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn claude_lowering_drops_approval_sandbox_mode_autocompact() {
        let content = "---\nname: coder\nharness: claude\napproval: auto\nsandbox: read-only\nmode: subagent\nautocompact: 50\nautocompact_pct: 80\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("approval:"), "approval leaked: {text}");
        assert!(!text.contains("sandbox:"), "sandbox leaked: {text}");
        assert!(!text.contains("autocompact:"), "autocompact leaked: {text}");
        // Lossiness should report dropped fields
        let dropped: Vec<_> = out.lossy_fields.iter().map(|f| f.field.as_str()).collect();
        assert!(
            dropped.contains(&"approval"),
            "approval not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"sandbox"),
            "sandbox not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"autocompact"),
            "autocompact not in lossy: {dropped:?}"
        );
        assert!(
            dropped.contains(&"autocompact_pct"),
            "autocompact_pct not in lossy: {dropped:?}"
        );
    }

    #[test]
    fn claude_harness_override_does_not_replace_skills_before_lowering() {
        let content = "---\nname: r\nharness: claude\nskills: [base-skill]\nharness-overrides:\n  claude:\n    skills: [override-skill]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("base-skill"),
            "top-level skill should be lowered: {text}"
        );
        assert!(
            !text.contains("override-skill"),
            "harness-overrides passthrough should not replace skills: {text}"
        );
    }

    #[test]
    fn claude_harness_override_does_not_replace_mcp() {
        let content = "---\nname: r\nharness: claude\ntools: [mcp(plugin:base)]\nharness-overrides:\n  claude:\n    mcp-tools: [plugin:claude]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("mcp-tools:"),
            "MCP grants belong in tools:, not a separate field: {text}"
        );
        assert!(
            text.contains("mcp__plugin:base__*"),
            "base mcp should project into tools: {text}"
        );
        assert!(
            !text.contains("plugin:claude"),
            "harness-overrides passthrough should not replace profile mcp: {text}"
        );
    }

    #[test]
    fn claude_agent_projects_mcp_refs_into_tools_and_disallowed() {
        let content = "---\nname: r\nharness: claude\ntools: [mcp(github/create_issue), mcp(context7)]\ndisallowed-tools: [mcp(github/delete_repo), mcp(*/cross_server_tool)]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("mcp__github__create_issue"),
            "per-tool MCP should land in tools: {text}"
        );
        assert!(
            text.contains("mcp__context7__*"),
            "whole-server MCP should land in tools: {text}"
        );
        assert!(
            text.contains("mcp__github__delete_repo"),
            "disallowed MCP should project into disallowed-tools: {text}"
        );
        assert!(
            !text.contains("mcp__*__cross_server_tool"),
            "cross-server MCP must not be emitted: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "disallowed-tools"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "Claude cannot scope a single MCP tool across all servers"
                    }
                )
        }));
    }

    #[test]
    fn pi_agent_records_mcp_lossiness_when_mcp_refs_present() {
        let content = "---\nname: r\nharness: pi\ntools: [mcp(plugin:demo)]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body(), &NativeModel::Inherit);
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "mcp"
                && field.target == "Pi"
                && matches!(field.classification, Lossiness::Dropped)
        }));
    }

    #[test]
    fn claude_meridian_only_fields_dropped() {
        let content = "---\nname: r\nharness: claude\nmodel-policies:\n  - match:\n      model: gpt55\n    override:\n      harness: codex\nfanout:\n  - alias: opus\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("model-policies:"),
            "model-policies leaked: {text}"
        );
        assert!(!text.contains("fanout:"), "fanout leaked: {text}");
        let meridian_only: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::MeridianOnly))
            .map(|f| f.field.as_str())
            .collect();
        assert!(meridian_only.contains(&"model-policies"));
        assert!(meridian_only.contains(&"fanout"));
    }

    #[test]
    fn claude_agent_model_invocable_false_warn_drops_without_skill_key() {
        let content = "---\nname: coder\nharness: claude\nmodel-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("disable-model-invocation"),
            "subagents have no invocation frontmatter; must not emit skill-only key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "model-invocable"
                && f.target == "Claude"
                && f.classification == Lossiness::Dropped
        }));
    }

    #[test]
    fn claude_agent_user_invocable_false_warn_drops() {
        let content = "---\nname: coder\nharness: claude\nuser-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, &fm, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("user-invocable"),
            "no native agent key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "user-invocable"
                && f.target == "Claude"
                && f.classification == Lossiness::Dropped
        }));
    }

    // --- 3.3: Codex lowering ---

    #[test]
    fn codex_lowering_produces_top_level_toml() {
        let content = "---\nname: coder\ndescription: Code agent\nmodel: gpt55\nharness: codex\neffort: high\nsandbox: workspace-write\napproval: auto\n---\n# Coder\nYou code.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("[agent]"),
            "legacy [agent] table leaked: {text}"
        );
        assert!(text.contains("name = \"coder\""), "name missing");
        assert!(text.contains("model = \"gpt55\""), "model missing");
        assert!(
            text.contains("model_reasoning_effort = \"high\""),
            "effort missing"
        );
        assert!(
            text.contains("sandbox_mode = \"workspace-write\""),
            "sandbox missing"
        );
        assert!(
            text.contains("approval_policy = \"on-request\""),
            "approval missing"
        );
        assert!(
            text.contains("developer_instructions ="),
            "developer instructions missing"
        );

        let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");
        assert!(
            parsed.get("agent").is_none(),
            "nested [agent] table present"
        );
        assert_eq!(parsed.get("name").and_then(|v| v.as_str()), Some("coder"));
    }

    #[test]
    fn codex_lowering_drops_skills_and_tools() {
        let content = "---\nname: r\nharness: codex\nskills: [review]\ntools: [Bash]\ndisallowed-tools: [Agent]\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let dropped: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::Dropped))
            .map(|f| f.field.as_str())
            .collect();
        assert!(dropped.contains(&"skills"));
        assert!(dropped.contains(&"tools"));
        assert!(dropped.contains(&"disallowed-tools"));
    }

    #[test]
    fn codex_harness_override_does_not_replace_execution_policy() {
        let content = "---\nname: r\nharness: codex\neffort: low\nharness-overrides:\n  codex:\n    effort: high\n    sandbox: workspace-write\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("model_reasoning_effort = \"low\""),
            "top-level effort should be lowered: {text}"
        );
        assert!(
            !text.contains("sandbox_mode = \"workspace-write\""),
            "harness-overrides passthrough should not replace sandbox: {text}"
        );
    }

    #[test]
    fn codex_mcp_lossiness_uses_top_level_policy() {
        let content = "---\nname: r\nharness: codex\ntools: [mcp(plugin:base)]\nharness-overrides:\n  codex:\n    mcp-tools: []\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            out.lossy_fields.iter().any(|field| field.field == "mcp"),
            "top-level mcp should remain lossy for codex: {:?}",
            out.lossy_fields
                .iter()
                .map(|field| field.field.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_native_config_lossiness_uses_matching_override() {
        let content = "---\nname: r\nharness-overrides:\n  codex:\n    native-config:\n      sandbox_workspace_write.network_access: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            out.lossy_fields.iter().any(|field| {
                field.field == "native-config"
                    && field.target == "Codex"
                    && matches!(field.classification, Lossiness::MeridianOnly)
            }),
            "native-config should be reported as meridian-only in codex lowering: {:?}",
            out.lossy_fields
                .iter()
                .map(|field| (&field.field, &field.target))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn codex_lowering_multiline_instructions_are_parseable() {
        let content = "---\nname: explorer\ndescription: \"Line one\\nLine two\"\nharness: codex\napproval: yolo\n---\n# Explore\nUse \"quotes\" and backslashes \\\\\nKeep going.";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        let parsed: toml::Value = toml::from_str(&text).expect("lowered TOML should parse");

        assert_eq!(
            parsed.get("approval_policy").and_then(|v| v.as_str()),
            Some("never")
        );
        assert_eq!(
            parsed
                .get("developer_instructions")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            "# Explore\nUse \"quotes\" and backslashes \\\\\nKeep going."
        );
    }

    // --- 3.3: OpenCode lowering ---

    #[test]
    fn opencode_lowering_preserves_name_description_model_mode() {
        let content = "---\nname: r\ndescription: Reviewer\nmodel: gpt55\nmode: primary\nharness: opencode\n---\n# Reviewer\nbody";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: r"), "name missing");
        assert!(text.contains("description: Reviewer"), "desc missing");
        assert!(text.contains("model: gpt55"), "model missing");
        assert!(text.contains("mode: primary"), "mode missing");
        assert!(
            !out.lossy_fields.iter().any(|f| f.field == "mode"),
            "mode should be exact, not lossy: {:?}",
            out.lossy_fields
        );
    }

    #[test]
    fn opencode_subagent_mode_emits_without_lossiness() {
        let content = "---\nname: sub\ndescription: Sub\nmode: subagent\nharness: opencode\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("mode: subagent"));
        assert!(
            !out.lossy_fields.iter().any(|f| f.field == "mode"),
            "mode should not be dropped or approximate: {:?}",
            out.lossy_fields
        );
    }

    #[test]
    fn claude_still_drops_mode() {
        let content = "---\nname: coder\nmode: subagent\nharness: claude\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_for_harness_with_model(
            &HarnessKind::Claude,
            &profile,
            &fm,
            fm.body(),
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(!text.contains("mode:"));
        assert!(
            out.lossy_fields.iter().any(|f| {
                f.field == "mode"
                    && f.target == "Claude"
                    && f.classification == Lossiness::Dropped
            }),
            "Claude should warn-drop mode: {:?}",
            out.lossy_fields
        );
    }

    #[test]
    fn cursor_lowering_uses_top_level_policy_and_matching_passthrough() {
        let content = "---\nname: r\nharness: cursor\ntools: [Read, mcp(plugin:base)]\nharness-overrides:\n  opencode:\n    tools: []\n    mcp-tools: []\n    native-config:\n      opencode.only: true\n  cursor:\n    tools: [Bash]\n    mcp-tools: [plugin:cursor]\n    native-config:\n      cursor.only: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let opencode = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field == "tools"),
            "harness-overrides passthrough should not clear tools lossiness",
        );
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field == "mcp"),
            "harness-overrides passthrough should not clear mcp lossiness",
        );

        let cursor = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|field| field.field == "tools"),
            "cursor override should keep tools lossiness",
        );
        assert!(
            cursor.lossy_fields.iter().any(|field| field.field == "mcp"),
            "cursor override should keep mcp lossiness",
        );
        assert!(
            cursor.lossy_fields.iter().any(|field| {
                field.field == "native-config"
                    && field.target == "Cursor"
                    && matches!(field.classification, Lossiness::MeridianOnly)
            }),
            "cursor native-config should be reported as meridian-only",
        );
    }

    #[test]
    fn cursor_lowering_normalizes_multiline_description_to_one_line() {
        let content = "---\nname: cursor-agent\ndescription: |\n  Cursor agent\n  with   lots\t of\n  whitespace\nharness: cursor\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            text.contains("description: Cursor agent with lots of whitespace")
                || text.contains("description: \"Cursor agent with lots of whitespace\""),
            "cursor description should be one line: {text}"
        );
        assert!(
            !text.contains("description: |\n"),
            "block description should be flattened: {text}"
        );
    }

    #[test]
    fn cursor_sandbox_is_approximate_not_dropped() {
        let content = "---\nname: r\nharness: cursor\nsandbox: read-only\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // sandbox must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("sandbox:"),
            "sandbox leaked into artifact: {text}"
        );

        // lossiness must be Approximate, not Dropped
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "sandbox")
            .expect("sandbox should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert!(
            matches!(field.classification, Lossiness::Approximate { .. }),
            "expected Approximate, got {:?}",
            field.classification
        );
        if let Lossiness::Approximate { note } = field.classification {
            assert!(
                note.contains("workspace-write") && note.contains("disabled"),
                "note should document workspace-write mapping: {note}"
            );
        }
    }

    #[test]
    fn cursor_approval_is_approximate_not_dropped() {
        let content = "---\nname: r\nharness: cursor\napproval: auto\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // approval must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("approval:"),
            "approval leaked into artifact: {text}"
        );

        // lossiness must be Approximate, not Dropped
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "approval")
            .expect("approval should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert!(
            matches!(field.classification, Lossiness::Approximate { .. }),
            "expected Approximate, got {:?}",
            field.classification
        );
        if let Lossiness::Approximate { note } = field.classification {
            assert!(
                note.contains("--force") && note.contains("--yolo"),
                "note should document --force/--yolo mapping: {note}"
            );
        }
    }

    // --- 3.3: Pi lowering ---

    #[test]
    fn pi_lowering_preserves_name_description_model() {
        let content = "---\nname: pi-agent\ndescription: Pi agent\nmodel: gpt55\nharness: pi\n---\n# Pi\nbody";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("name: pi-agent"), "name missing");
        assert!(text.contains("description: Pi agent"), "desc missing");
    }

    #[test]
    fn lower_for_harness_with_model_field_emits_pinned_id_for_claude_and_codex() {
        let claude_content = "---\nname: coder\nmodel: gpt55\nharness: claude\n---\n# Coder\nbody";
        let (claude_profile, claude_fm, _) = profile_from(claude_content);
        let claude_out = lower_for_harness_with_model(
            &HarnessKind::Claude,
            &claude_profile,
            &claude_fm,
            claude_fm.body(),
            &NativeModel::Set("o3".to_string()),
        );
        let claude_text = String::from_utf8(claude_out.bytes).unwrap();
        assert!(
            claude_text.contains("model: o3"),
            "claude override: {claude_text}"
        );
        assert!(
            !claude_text.contains("model: gpt55"),
            "alias should not leak when override set: {claude_text}"
        );

        let codex_content = "---\nname: coder\nmodel: gpt55\nharness: codex\n---\n# body";
        let (codex_profile, codex_fm, _) = profile_from(codex_content);
        let codex_out = lower_for_harness_with_model(
            &HarnessKind::Codex,
            &codex_profile,
            &codex_fm,
            codex_fm.body(),
            &NativeModel::Set("o3".to_string()),
        );
        let codex_text = String::from_utf8(codex_out.bytes).unwrap();
        assert!(
            codex_text.contains("model = \"o3\""),
            "codex override: {codex_text}"
        );
        assert!(
            !codex_text.contains("model = \"gpt55\""),
            "alias should not leak when override set: {codex_text}"
        );
    }

    // --- 3.3: Dispatch ---

    #[test]
    fn lower_for_harness_dispatches_correctly() {
        let content = "---\nname: coder\nmodel: gpt55\nharness: claude\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let body = fm.body().to_string();
        let out = lower_for_harness_with_model(
            &HarnessKind::Claude,
            &profile,
            &fm,
            &body,
            &NativeModel::Inherit,
        );
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("---"), "not markdown format");

        let content2 = "---\nname: coder\nmodel: gpt55\nharness: codex\n---\n# body";
        let (profile2, fm2, _) = profile_from(content2);
        let body2 = fm2.body().to_string();
        let out2 = lower_for_harness_with_model(
            &HarnessKind::Codex,
            &profile2,
            &fm2,
            &body2,
            &NativeModel::Inherit,
        );
        let text2 = String::from_utf8(out2.bytes).unwrap();
        assert!(text2.contains("name = \"coder\""), "not TOML format");
        assert!(
            !text2.contains("[agent]"),
            "legacy nested agent table emitted"
        );

        let content3 = "---\nname: cursor-agent\nmodel: gpt55\nharness: cursor\n---\n# body";
        let (profile3, fm3, _) = profile_from(content3);
        let out3 = lower_for_harness_with_model(
            &HarnessKind::Cursor,
            &profile3,
            &fm3,
            fm3.body(),
            &NativeModel::Inherit,
        );
        let text3 = String::from_utf8(out3.bytes).unwrap();
        assert!(
            text3.contains("name: cursor-agent"),
            "cursor lowering missing name"
        );
    }
}
