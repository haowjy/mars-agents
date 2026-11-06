//! Public entry points and tests for per-harness agent lowering.
//!
//! The implementation lives in `lower_policy.rs` so the declarative policy table
//! and shared lowering pipeline stay separate from the regression tests.

#[path = "lower_policy.rs"]
mod lower_policy;

pub use lower_policy::{NativeModel, lower_for_harness_with_model};

#[cfg(test)]
#[cfg(test)]
pub use lower_policy::{
    lower_to_claude, lower_to_codex, lower_to_cursor_with_model, lower_to_opencode, lower_to_pi,
};

#[cfg(test)]
use crate::compiler::lossiness::Lossiness;

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::{ frontmatter::Frontmatter};
    use mars_schema::{agent::AgentProfile, diagnostic::SchemaDiagnostic};

    fn profile_from(content: &str) -> (AgentProfile, Frontmatter, Vec<SchemaDiagnostic>) {
        let reading = mars_schema::agent::read(content);
        let fm = Frontmatter::parse(content).unwrap();
        (reading.fields.unwrap(), fm, reading.diagnostics)
    }

    // --- 3.3: Claude lowering ---

    #[test]
    fn claude_lowering_omits_unrecognized_tools_and_preserves_mcp_wire_identifiers() {
        let content = "---\nname: coder\nharness: claude\ntools:\n  '*': deny\n  my_custom_tool: allow\n  mcp__server__Tool: allow\n---\n# Body";
        let (profile, fm, diags) = profile_from(content);
        assert!(diags.is_empty());
        let out = lower_to_claude(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();

        assert!(
            !text.to_lowercase().contains("custom"),
            "an unrecognized tool must not be guessed into a native name: {text}"
        );
        assert!(
            text.contains("- mcp__server__Tool"),
            "mcp__ wire identifier should be preserved verbatim: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "tools"
                && field.target == "Claude"
                && matches!(
                    field.classification,
                    Lossiness::Approximate {
                        note: "no native name known for this tool on this harness; left out"
                    }
                )
        }));
    }

    #[test]
    fn claude_lowering_drops_approval_sandbox_network_mode() {
        let content = "---\nname: coder\nharness: claude\napproval: auto\nsandbox: read-only\nnetwork: allowlist\nnetwork-allow: [example.com]\nnetwork-deny: [blocked.example]\nmode: subagent\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        for leaked in [
            "approval:",
            "sandbox:",
            "network:",
            "network-allow:",
            "network-deny:",
        ] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
        // Launch-time fields are meridian-only, not target-enforced drops.
        let meridian_only: Vec<_> = out
            .lossy_fields
            .iter()
            .filter(|f| matches!(f.classification, Lossiness::MeridianOnly))
            .map(|f| f.field.as_str())
            .collect();
        for field in [
            "approval",
            "sandbox",
            "network",
            "network-allow",
            "network-deny",
            "mode",
        ] {
            assert!(
                meridian_only.contains(&field),
                "{field} not in lossy: {meridian_only:?}"
            );
        }
    }

    #[test]
    fn claude_harness_override_does_not_replace_mcp() {
        let content = "---\nname: r\nharness: claude\ntools:\n  '*': deny\n  mcp(plugin:base): allow\nharness-overrides:\n  claude:\n    tools: [plugin:claude]\n    color: red\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("color: red"),
            "non-conflicting override should be written: {text}"
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
    fn pi_agent_records_mcp_lossiness_when_mcp_refs_present() {
        let content = "---\nname: r\nharness: pi\ntools:\n  mcp(plugin:demo): allow\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_pi(&profile, fm.body(), &NativeModel::Inherit);
        assert!(out.lossy_fields.iter().any(|field| {
            field.field.starts_with("mcp")
                && field.target == "Pi"
                && matches!(field.classification, Lossiness::Quiet { .. })
        }));
    }
    #[test]
    fn claude_agent_model_invocable_false_warn_drops_without_skill_key() {
        let content = "---\nname: coder\nharness: claude\nmodel-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("disable-model-invocation"),
            "subagents have no invocation frontmatter; must not emit skill-only key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "model-invocable"
                && f.target == "Claude"
                && matches!(f.classification, Lossiness::Quiet { .. })
        }));
    }

    #[test]
    fn claude_agent_user_invocable_false_warn_drops() {
        let content = "---\nname: coder\nharness: claude\nuser-invocable: false\n---\n# Body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_claude(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("user-invocable"),
            "no native agent key: {text}"
        );
        assert!(out.lossy_fields.iter().any(|f| {
            f.field == "user-invocable"
                && f.target == "Claude"
                && matches!(f.classification, Lossiness::Quiet { .. })
        }));
    }

    // --- 3.3: Codex lowering ---

    #[test]
    fn codex_lowering_drops_network_and_keeps_sandbox() {
        let content = "---\nname: coder\nharness: codex\nsandbox: workspace-write\nnetwork: allowlist\nnetwork-allow: [example.com]\nnetwork-deny: [blocked.example]\n---\n# Coder";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            text.contains("sandbox_mode = \"workspace-write\""),
            "sandbox missing: {text}"
        );
        assert!(!text.contains("network"), "network leaked: {text}");
        for field in ["network", "network-allow", "network-deny"] {
            assert!(
                out.lossy_fields.iter().any(|lossy| lossy.field == field
                    && matches!(lossy.classification, Lossiness::Dropped(_))),
                "{field} not reported as a lost restriction: {:?}",
                out.lossy_fields
            );
        }
    }

    #[test]
    fn codex_mcp_lossiness_uses_top_level_policy() {
        let content = "---\nname: r\nharness: codex\ntools:\n  mcp(plugin:base): allow\nharness-overrides:\n  codex:\n    mcp-tools: []\n---\n# body";
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
    fn codex_override_adds_keys_and_never_replaces_lowered_ones() {
        let content = "---\nname: r\neffort: low\nharness-overrides:\n  codex:\n    custom_key: 1\n    model_reasoning_effort: high\n  claude:\n    color: red\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_codex(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(text.contains("custom_key = 1"), "{text}");
        assert!(text.contains("model_reasoning_effort = \"low\""), "{text}");
        assert!(
            !text.contains("color"),
            "claude overrides must not reach Codex: {text}"
        );
        assert!(out.lossy_fields.iter().any(|field| {
            field.field == "harness-overrides.codex.model_reasoning_effort"
                && matches!(field.classification, Lossiness::Quiet { .. })
        }));
    }

    // --- 3.3: OpenCode lowering ---

    #[test]
    fn cursor_lowering_uses_top_level_policy_and_matching_passthrough() {
        let content = "---\nname: r\nharness: cursor\ntools:\n  \"*\": deny\n  Read: allow\n  mcp(plugin:base): allow\nharness-overrides:\n  opencode:\n    tools: []\n    mcp-tools: []\n    native-config:\n      opencode.only: true\n  cursor:\n    tools: [Bash]\n    mcp-tools: [plugin:cursor]\n    native-config:\n      cursor.only: true\n---\n# body";
        let (profile, fm, _) = profile_from(content);

        let opencode = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|field| field.field.starts_with("tools")),
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
                .any(|field| field.field.starts_with("tools")),
            "cursor override should keep tools lossiness",
        );
        assert!(
            cursor.lossy_fields.iter().any(|field| field.field == "mcp"),
            "cursor override should keep mcp lossiness",
        );
        assert!(
            String::from_utf8(cursor.bytes)
                .unwrap()
                .contains("native-config:")
                && !String::from_utf8(opencode.bytes)
                    .unwrap()
                    .contains("cursor.only"),
            "each harness gets only its own override block",
        );
    }

    #[test]
    fn cursor_sandbox_is_meridian_only() {
        let content = "---\nname: r\nharness: cursor\nsandbox: read-only\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // sandbox must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("sandbox:"),
            "sandbox leaked into artifact: {text}"
        );

        // Meridian applies it at spawn, like every other target
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "sandbox")
            .expect("sandbox should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert_eq!(field.classification, Lossiness::MeridianOnly);
    }

    #[test]
    fn cursor_approval_is_meridian_only() {
        let content = "---\nname: r\nharness: cursor\napproval: auto\n---\n# body";
        let (profile, fm, _) = profile_from(content);
        let out = lower_to_cursor_with_model(&profile, fm.body(), &NativeModel::Inherit);

        // approval must not appear in the emitted YAML artifact
        let text = String::from_utf8(out.bytes).unwrap();
        assert!(
            !text.contains("approval:"),
            "approval leaked into artifact: {text}"
        );

        // Meridian applies it at spawn, like every other target
        let field = out
            .lossy_fields
            .iter()
            .find(|f| f.field == "approval")
            .expect("approval should appear in lossy_fields");
        assert_eq!(field.target, "Cursor");
        assert_eq!(field.classification, Lossiness::MeridianOnly);
    }

    // --- 3.3: Dispatch ---
}
