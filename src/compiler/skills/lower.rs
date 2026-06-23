//! Per-harness lowering for universal skill frontmatter.

#[path = "lower_policy.rs"]
mod lower_policy;

use crate::compiler::lossiness::LoweredOutput;
use crate::compiler::skills::SkillProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillHarness {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Cursor,
}

impl SkillHarness {
    pub fn from_variant_key(key: &str) -> Option<Self> {
        match key {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }
}

pub fn lower_skill_for_harness(
    harness: SkillHarness,
    profile: &SkillProfile,
    body: &str,
) -> LoweredOutput {
    match harness {
        SkillHarness::Claude => lower_skill_to_claude(profile, body),
        SkillHarness::Codex => lower_skill_to_codex(profile, body),
        SkillHarness::OpenCode => lower_skill_to_opencode(profile, body),
        SkillHarness::Pi => lower_skill_to_pi(profile, body),
        SkillHarness::Cursor => lower_skill_to_cursor(profile, body),
    }
}

pub fn lower_skill_to_claude(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(SkillHarness::Claude, profile, body)
}

pub fn lower_skill_to_codex(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(SkillHarness::Codex, profile, body)
}

pub fn lower_skill_to_opencode(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(SkillHarness::OpenCode, profile, body)
}

pub fn lower_skill_to_pi(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(SkillHarness::Pi, profile, body)
}

pub fn lower_skill_to_cursor(profile: &SkillProfile, body: &str) -> LoweredOutput {
    lower_policy::lower_skill_with_policy(SkillHarness::Cursor, profile, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::lossiness::{Lossiness, LossyField};
    use crate::compiler::skills::parse_skill_content;

    fn parse_profile(content: &str) -> SkillProfile {
        let mut diags = Vec::new();
        parse_skill_content(content, &mut diags).unwrap().0
    }

    fn profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\ntools: [Bash(git *)]\nlicense: MIT\nmetadata:\n  owner: team\nextra: stripped\n---\nBody\n",
        )
    }

    fn identity_profile() -> SkillProfile {
        parse_profile("---\nname: skill\ndescription: desc\n---\nBody\n")
    }

    fn user_invocable_false_profile() -> SkillProfile {
        parse_profile("---\nname: skill\ndescription: desc\nuser-invocable: false\n---\nBody\n")
    }

    fn explicit_true_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: true\nuser-invocable: true\n---\nBody\n",
        )
    }

    fn both_false_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\nmodel-invocable: false\nuser-invocable: false\n---\nBody\n",
        )
    }

    fn has_dropped(lossy_fields: &[LossyField], field: &str, target: &str) -> bool {
        lossy_fields.iter().any(|f| {
            f.field == field && f.target == target && f.classification == Lossiness::Dropped
        })
    }

    fn disallowed_tools_profile() -> SkillProfile {
        parse_profile(
            "---\nname: skill\ndescription: desc\ndisallowed-tools: [Agent, Bash(git *)]\n---\nBody\n",
        )
    }

    #[test]
    fn claude_emits_disallowed_tools_projected() {
        let lowered = lower_skill_to_claude(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"));
        assert!(out.contains("- Agent"));
        assert!(
            out.contains("- Bash(git *)"),
            "scoped payload preserved: {out}"
        );
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn pi_emits_disallowed_tools() {
        let lowered = lower_skill_to_pi(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"));
        assert!(out.contains("- agent"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Pi"
        ));
    }

    #[test]
    fn codex_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_codex(&disallowed_tools_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disallowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Codex"
        ));
    }

    #[test]
    fn opencode_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_opencode(&disallowed_tools_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "OpenCode"
        ));
    }

    #[test]
    fn cursor_warn_drops_disallowed_tools() {
        let lowered = lower_skill_to_cursor(&disallowed_tools_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Cursor"
        ));
    }

    #[test]
    fn claude_maps_model_invocation_and_tools() {
        let lowered = lower_skill_to_claude(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
        assert!(out.contains("allowed-tools:"));
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(out.contains("license: MIT"));
        assert!(out.contains("owner: team"));
        // Unknown fields pass through to all targets
        assert!(out.contains("extra: stripped"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_projects_canonical_allowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools: [AskUser, Bash(git *)]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();

        assert!(out.contains("- AskUser"), "AskUser not projected: {out}");
        assert!(
            out.contains("- Bash(git *)"),
            "scoped Bash not projected while preserving payload: {out}"
        );
    }

    #[test]
    fn claude_lowers_unknown_custom_tools_and_preserves_mcp_wire_identifiers() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools: [my_custom_tool, mcp__server__Tool]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();

        assert!(
            out.contains("- MyCustomTool"),
            "unknown custom tool should be convention-projected: {out}"
        );
        assert!(
            out.contains("- mcp__server__Tool"),
            "mcp__ wire identifier should be preserved verbatim: {out}"
        );
        assert!(lowered.lossy_fields.iter().any(|field| {
            field.field == "tools"
                && field.target == "Claude"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "unknown tool projected via harness naming convention"
                    }
                )
        }));
    }

    #[test]
    fn claude_lowers_tools_map_allow_and_deny() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  ask_user: allow\n  \"bash(git *)\": deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("allowed-tools:"));
        assert!(out.contains("- AskUser"));
        assert!(
            !out.contains("bash(git *)"),
            "denied tools must not appear in allowlist: {out}"
        );
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_emits_user_invocable_false() {
        let lowered = lower_skill_to_claude(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("user-invocable: false"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn claude_omits_user_invocable_when_true() {
        let out =
            String::from_utf8(lower_skill_to_claude(&explicit_true_profile(), "Body\n").bytes)
                .unwrap();
        assert!(!out.contains("user-invocable"));
    }

    #[test]
    fn claude_omits_disable_model_invocation_when_true() {
        let out =
            String::from_utf8(lower_skill_to_claude(&explicit_true_profile(), "Body\n").bytes)
                .unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!out.contains("allow_implicit_invocation"));
    }

    #[test]
    fn claude_both_false() {
        let lowered = lower_skill_to_claude(&both_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
        assert!(out.contains("user-invocable: false"));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn codex_warn_drops_model_invocable_and_tools() {
        let lowered = lower_skill_to_codex(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("allowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
        assert!(has_dropped(&lowered.lossy_fields, "tools", "Codex"));
    }

    #[test]
    fn codex_identity_only_does_not_gain_invocation_field() {
        let out =
            String::from_utf8(lower_skill_to_codex(&identity_profile(), "Body\n").bytes).unwrap();
        assert!(out.contains("name: skill"));
        assert!(out.contains("description: desc"));
        assert!(!out.contains("allow_implicit_invocation"));
    }

    #[test]
    fn codex_explicit_true_warn_drops_model_invocable() {
        let lowered = lower_skill_to_codex(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("allow_implicit_invocation"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Codex"
        ));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn codex_drops_user_invocable_false() {
        let lowered = lower_skill_to_codex(&user_invocable_false_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn codex_no_lossiness_user_invocable_true() {
        let lowered = lower_skill_to_codex(&identity_profile(), "Body\n");
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Codex"
        ));
    }

    #[test]
    fn opencode_drops_model_invocable_and_tools() {
        let lowered = lower_skill_to_opencode(&profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "OpenCode"
        ));
        assert!(has_dropped(&lowered.lossy_fields, "tools", "OpenCode"));
        assert_eq!(lowered.lossy_fields.len(), 2);
    }

    #[test]
    fn opencode_drops_user_invocable_false() {
        let lowered = lower_skill_to_opencode(&user_invocable_false_profile(), "Body\n");
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
    }

    #[test]
    fn opencode_no_invocability_lossiness_when_defaults() {
        let lowered = lower_skill_to_opencode(&identity_profile(), "Body\n");
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "OpenCode"
        ));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "OpenCode"
        ));
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn pi_model_false_emits_disable_model_invocation() {
        let lowered = lower_skill_to_pi(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disable-model-invocation: true"));
    }

    #[test]
    fn pi_drops_user_invocable_false() {
        let lowered = lower_skill_to_pi(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(has_dropped(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn pi_model_true_omits_disable_model_invocation_and_user_true_no_lossiness() {
        let lowered = lower_skill_to_pi(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!has_dropped(&lowered.lossy_fields, "user-invocable", "Pi"));
    }

    #[test]
    fn codex_warn_drops_when_to_use_without_emitting() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nwhen_to_use: Use for git\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("when_to_use"));
        assert!(has_dropped(&lowered.lossy_fields, "when_to_use", "Codex"));
    }

    #[test]
    fn cursor_drops_tools() {
        let lowered = lower_skill_to_cursor(&profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("allowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "model-invocable",
            "Cursor"
        ));
        assert_eq!(lowered.lossy_fields.len(), 2);
    }

    #[test]
    fn cursor_drops_user_invocable_false() {
        let lowered = lower_skill_to_cursor(&user_invocable_false_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("user-invocable"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn cursor_model_true_emits_always_apply_not_claude_keys() {
        let lowered = lower_skill_to_cursor(&explicit_true_profile(), "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("alwaysApply: true"));
        assert!(!out.contains("disable-model-invocation"));
        assert!(!out.contains("user-invocable"));
        assert!(!has_dropped(
            &lowered.lossy_fields,
            "user-invocable",
            "Cursor"
        ));
    }

    #[test]
    fn snake_case_model_invocable_not_leaked_to_any_target() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nmodel_invocable: false\n---\nBody\n",
        );
        for harness in [
            SkillHarness::Claude,
            SkillHarness::Codex,
            SkillHarness::OpenCode,
            SkillHarness::Pi,
            SkillHarness::Cursor,
        ] {
            let lowered = lower_skill_for_harness(harness, &profile, "Body\n");
            let out = String::from_utf8(lowered.bytes).unwrap();
            assert!(
                !out.contains("model_invocable"),
                "leaked snake key for {harness:?}: {out}"
            );
        }
        let claude = String::from_utf8(lower_skill_to_claude(&profile, "Body\n").bytes).unwrap();
        assert!(claude.contains("disable-model-invocation: true"));
    }

    #[test]
    fn no_frontmatter_body_only_all_harnesses() {
        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content("# Body\nbytes", &mut diags).unwrap();
        let body = fm.body();
        for harness in [
            SkillHarness::Claude,
            SkillHarness::Codex,
            SkillHarness::OpenCode,
            SkillHarness::Pi,
            SkillHarness::Cursor,
        ] {
            let out =
                String::from_utf8(lower_skill_for_harness(harness, &profile, body).bytes).unwrap();
            assert_eq!(out, "# Body\nbytes");
        }
    }

    #[test]
    fn claude_lowers_tools_map_deny_to_disallowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  Agent: deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(out.contains("disallowed-tools:"), "missing denylist: {out}");
        assert!(out.contains("- Agent"), "Agent not projected: {out}");
        assert!(lowered.lossy_fields.is_empty());
    }

    #[test]
    fn codex_warns_tools_map_deny_via_disallowed_tools() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools:\n  Agent: deny\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(!out.contains("disallowed-tools"));
        assert!(has_dropped(
            &lowered.lossy_fields,
            "disallowed-tools",
            "Codex"
        ));
    }

    #[test]
    fn canonical_allowed_tools_not_emitted_in_claude_lower() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\nallowed-tools: [Bash]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(
            !out.contains("allowed-tools"),
            "flat allowed-tools must not pass through: {out}"
        );
        assert!(
            !profile
                .passthrough_fields
                .iter()
                .any(|(k, _)| k == "allowed-tools")
        );
    }

    #[test]
    fn claude_emits_mcp() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools: [mcp(plugin:demo)]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(
            out.contains("allowed-tools:"),
            "MCP grants belong in allowed-tools: {out}"
        );
        assert!(
            out.contains("mcp__plugin:demo__*"),
            "missing projected mcp entry: {out}"
        );
        assert!(
            !out.contains("mcp-tools:"),
            "must not emit a separate mcp field: {out}"
        );
        assert!(lowered.lossy_fields.iter().any(|field| {
            field.field == "mcp" && matches!(field.classification, Lossiness::Approximate { .. })
        }));
    }

    #[test]
    fn claude_emits_disallowed_mcp_ref_in_denylist() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ndisallowed-tools: [mcp(github/delete_repo)]\n---\nBody\n",
        );
        let lowered = lower_skill_to_claude(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(
            out.contains("mcp__github__delete_repo"),
            "disallowed MCP ref should project into disallowed-tools: {out}"
        );
        assert!(
            !out.contains("mcp-tools:"),
            "must not emit a separate mcp field: {out}"
        );
    }

    #[test]
    fn codex_warns_mcp() {
        let profile = parse_profile(
            "---\nname: skill\ndescription: desc\ntools: [mcp(plugin:demo)]\n---\nBody\n",
        );
        let lowered = lower_skill_to_codex(&profile, "Body\n");
        let out = String::from_utf8(lowered.bytes).unwrap();
        assert!(
            !out.contains("mcp-tools"),
            "separate mcp field must not emit: {out}"
        );
        assert!(
            lowered
                .lossy_fields
                .iter()
                .any(|field| field.field == "mcp"),
            "expected mcp lossiness: {:?}",
            lowered.lossy_fields
        );
    }
}
