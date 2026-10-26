//! Binary adapter for schema-owned foreign frontmatter lifting.

use mars_schema::frontmatter::Frontmatter;
use mars_schema::item::ItemKind;

use crate::dialect::Dialect;

pub(crate) fn lift_frontmatter_with_change(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> (Frontmatter, bool) {
    let dialect = match dialect {
        Dialect::Claude => mars_schema::dialect::Dialect::Claude,
        Dialect::Codex => mars_schema::dialect::Dialect::Codex,
        Dialect::OpenCode => mars_schema::dialect::Dialect::OpenCode,
        Dialect::Cursor => mars_schema::dialect::Dialect::Cursor,
        Dialect::MarsNative => mars_schema::dialect::Dialect::MarsNative,
    };
    mars_schema::lift::lift_frontmatter_with_change(dialect, item_kind, frontmatter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::lower::{NativeModel, lower_to_claude};
    use crate::compiler::lossiness::Lossiness;
    use crate::compiler::skills::lower::{SkillHarness, lower_skill_for_harness};
    use mars_schema::agent::parse_agent_content;
    use mars_schema::frontmatter::Frontmatter;
    use mars_schema::skill::parse_skill_content;
    use serde_yaml::Value;

    fn fm(yaml: &str) -> Frontmatter {
        Frontmatter::parse(&format!("---\n{yaml}---\n# Body\n")).unwrap()
    }

    fn lift(dialect: Dialect, item_kind: ItemKind, frontmatter: &Frontmatter) -> Frontmatter {
        lift_frontmatter_with_change(dialect, item_kind, frontmatter).0
    }

    fn tool_map(entries: &[(&str, &str)]) -> Value {
        Value::Mapping(
            entries
                .iter()
                .map(|(name, action)| {
                    (
                        Value::String((*name).into()),
                        Value::String((*action).into()),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn claude_skill_disable_model_invocation_inverts_to_model_invocable() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Skill,
            &fm("name: s\ndescription: d\ndisable-model-invocation: true\n"),
        );
        assert_eq!(lifted.get("model-invocable"), Some(&Value::Bool(false)));
        assert!(lifted.get("disable-model-invocation").is_none());
    }

    #[test]
    fn claude_skill_idempotent_on_existing_model_invocable() {
        let input = fm("name: s\ndescription: d\nmodel-invocable: false\n");
        let lifted = lift(Dialect::Claude, ItemKind::Skill, &input);
        assert!(lifted.get("model-invocable").is_some());
        assert!(lifted.get("disable-model-invocation").is_none());
    }

    #[test]
    fn claude_agent_removes_duplicate_foreign_keys_when_canonical_present() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ndisallowed-tools: [Read]\ndisallowedTools: [Agent]\ntools: [mcp(server)]\nmcpServers: [other]\n",
            ),
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("Read".into())]))
        );
        assert!(lifted.get("disallowedTools").is_none());
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("*", "deny"),
                ("mcp(server)", "allow"),
                ("mcp(other)", "allow"),
            ]))
        );
        assert!(lifted.get("mcpServers").is_none());
    }

    #[test]
    fn claude_agent_disallowed_tools_camel_case() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\ndisallowedTools: [Agent]\n"),
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("Agent".into())]))
        );
        assert!(lifted.get("disallowedTools").is_none());
    }

    #[test]
    fn opencode_primary_mode_preserves_mode_without_inferring_user_invocable() {
        let lifted = lift(
            Dialect::OpenCode,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\nmode: primary\n"),
        );
        assert_eq!(lifted.get("mode"), Some(&Value::String("primary".into())));
        assert!(lifted.get("user-invocable").is_none());
    }

    #[test]
    fn opencode_agent_lifts_literal_user_invocable_only() {
        let lifted = lift(
            Dialect::OpenCode,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\nmode: subagent\nuser-invocable: false\n"),
        );
        assert_eq!(lifted.get("user-invocable"), Some(&Value::Bool(false)));
        assert_eq!(lifted.get("mode"), Some(&Value::String("subagent".into())));
    }

    #[test]
    fn cursor_always_apply_lifts_model_invocable() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("description: rule\nalwaysApply: true\nglobs: \"**/*\"\n"),
        );
        assert_eq!(lifted.get("model-invocable"), Some(&Value::Bool(true)));
        assert!(lifted.get("alwaysApply").is_none());
        assert!(lifted.get("globs").is_none());
    }

    #[test]
    fn cursor_manual_rule_restores_model_invocable_false_without_description() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("name: demo\nalwaysApply: false\n"),
        );
        assert_eq!(lifted.get("model-invocable"), Some(&Value::Bool(false)));
        assert!(lifted.get("description").is_none());
        assert!(lifted.get("alwaysApply").is_none());

        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content(&lifted.render(), &mut diags).unwrap();
        assert!(
            diags
                .iter()
                .any(|diagnostic| diagnostic.code == "field.required")
        );
        assert!(profile.description.is_none());
        assert!(!profile.model_invocable);
        assert!(profile.had_model_invocable_field);

        let body = "# Demo\n";
        let codex = lower_skill_for_harness(SkillHarness::Codex, &profile, body);
        assert_eq!(codex.siblings.len(), 1);
        assert!(
            !codex
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable" && f.classification == Lossiness::Dropped)
        );

        let claude = lower_skill_for_harness(SkillHarness::Claude, &profile, body);
        let claude_out = String::from_utf8(claude.bytes).unwrap();
        assert!(
            !claude_out.contains("description:"),
            "fabricated description must not leak to Claude: {claude_out}"
        );

        let opencode = lower_skill_for_harness(SkillHarness::OpenCode, &profile, body);
        let opencode_out = String::from_utf8(opencode.bytes).unwrap();
        assert!(
            !opencode_out.contains("description:"),
            "fabricated description must not leak to OpenCode: {opencode_out}"
        );

        let cursor = lower_skill_for_harness(SkillHarness::Cursor, &profile, body);
        let cursor_out = String::from_utf8(cursor.bytes).unwrap();
        assert!(!cursor_out.contains("description:"));
        assert!(cursor_out.contains("alwaysApply: false"));

        let round_trip_lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &Frontmatter::parse(&cursor_out).unwrap(),
        );
        assert_eq!(
            round_trip_lifted.get("model-invocable"),
            Some(&Value::Bool(false))
        );
        assert!(round_trip_lifted.get("description").is_none());
    }

    #[test]
    fn cursor_apply_intelligently_does_not_set_model_invocable_false() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("name: demo\ndescription: Pick me\nalwaysApply: false\n"),
        );
        assert!(lifted.get("model-invocable").is_none());
        assert_eq!(
            lifted.get("description"),
            Some(&Value::String("Pick me".into()))
        );
        assert!(lifted.get("alwaysApply").is_none());
    }

    #[test]
    fn claude_agent_lifts_task_tool_to_agent_in_every_tool_field() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ntools: [Read, Task, 'Task(Explore)']\ndisallowedTools: [Task]\n",
            ),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("*", "deny"),
                ("Read", "allow"),
                ("agent", "allow"),
                ("agent(Explore)", "allow"),
            ]))
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("agent".into())]))
        );

        // Claude's own agent files spell lists as one comma-separated scalar.
        let scalar = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ntools: Read, Task, mcp__github__get_issue\ndisallowedTools: Task, Edit\n",
            ),
        );
        assert_eq!(
            scalar.get("tools"),
            Some(&tool_map(&[
                ("*", "deny"),
                ("Read", "allow"),
                ("agent", "allow"),
                ("mcp(github/get_issue)", "allow"),
            ]))
        );
        assert_eq!(
            scalar.get("disallowed-tools"),
            Some(&Value::Sequence(vec![
                Value::String("agent".into()),
                Value::String("Edit".into()),
            ]))
        );
    }

    #[test]
    fn claude_agent_tool_list_round_trips_as_a_restriction_without_deprecation() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\ntools: Read, Grep\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("*", "deny"),
                ("Read", "allow"),
                ("Grep", "allow"),
            ]))
        );

        let mut diags = Vec::new();
        let (profile, canonical) = parse_agent_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");

        let output = lower_to_claude(
            &profile,
            &canonical,
            canonical.body(),
            &NativeModel::Inherit,
        );
        assert!(output.lossy_fields.is_empty(), "{:?}", output.lossy_fields);
        let lowered = Frontmatter::parse(&String::from_utf8(output.bytes).unwrap()).unwrap();
        assert_eq!(
            lowered.get("tools"),
            Some(&Value::Sequence(vec![
                Value::String("Read".into()),
                Value::String("Grep".into()),
            ]))
        );
    }

    #[test]
    fn claude_agent_lifts_whole_server_and_global_mcp_wire_tokens() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\nallowed-tools: [mcp__*, mcp__github]\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("mcp(*/*)", "allow"),
                ("mcp(github/*)", "allow"),
            ]))
        );
    }

    #[test]
    fn claude_lifts_foreign_mcp_token_in_map_form_tools_key() {
        // Map-form `tools:` carries the MCP token in the key. The deny value and
        // any non-MCP key must survive unchanged.
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ntools:\n  mcp__github__delete_repo: deny\n  Read: allow\n",
            ),
        );
        let mut expected = serde_yaml::Mapping::new();
        expected.insert(
            Value::String("mcp(github/delete_repo)".into()),
            Value::String("deny".into()),
        );
        expected.insert(Value::String("Read".into()), Value::String("allow".into()));
        assert_eq!(lifted.get("tools"), Some(&Value::Mapping(expected)));
    }

    #[test]
    fn claude_agent_lifts_scalar_foreign_mcp_token_in_disallowed_tools() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\ndisallowedTools: mcp__github__delete_repo\n"),
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::String("mcp(github/delete_repo)".into()))
        );
    }

    #[test]
    fn claude_agent_merges_allowed_tools_before_mcp_servers() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\nallowed-tools: [Read, Bash(git *)]\nmcpServers: [context7, github]\n",
            ),
        );
        assert!(lifted.get("allowed-tools").is_none());
        assert!(lifted.get("mcpServers").is_none());
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("Read", "allow"),
                ("Bash(git *)", "allow"),
                ("mcp(context7)", "allow"),
                ("mcp(github)", "allow"),
            ]))
        );

        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools.describe(),
            [
                "allow:read",
                "allow:bash(git *)",
                "allow:mcp(context7/*)",
                "allow:mcp(github/*)"
            ]
        );
    }

    #[test]
    fn claude_agent_appends_mcp_servers_to_map_form_tools() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ntools:\n  Bash: allow\n  Read: deny\nmcpServers: [context7]\n",
            ),
        );
        let tools = lifted.get("tools").unwrap();
        let Value::Mapping(mapping) = tools else {
            panic!("expected map-form tools, got {tools:?}");
        };
        assert_eq!(
            mapping.get(Value::String("Bash".into())),
            Some(&Value::String("allow".into()))
        );
        assert_eq!(
            mapping.get(Value::String("Read".into())),
            Some(&Value::String("deny".into()))
        );
        assert_eq!(
            mapping.get(Value::String("mcp(context7)".into())),
            Some(&Value::String("allow".into()))
        );
        assert!(lifted.get("mcpServers").is_none());
    }
}
