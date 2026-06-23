//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::compiler::invocability::find_invocability_field;
use crate::compiler::mcp_ref::parse_foreign_mcp_token;
use crate::compiler::tool_policy;
use crate::dialect::Dialect;
use crate::frontmatter::Frontmatter;
use crate::lock::ItemKind;

fn lift_foreign_mcp_tokens_in_value(value: &Value, dialect: Dialect) -> (Value, bool) {
    match value {
        Value::String(raw) => {
            let Some(mcp_ref) = parse_foreign_mcp_token(raw, dialect) else {
                return (value.clone(), false);
            };
            (Value::String(mcp_ref.to_canonical()), true)
        }
        Value::Sequence(seq) => {
            let mut changed = false;
            let lifted: Vec<Value> = seq
                .iter()
                .map(|entry| {
                    let Some(raw) = entry.as_str() else {
                        return entry.clone();
                    };
                    let Some(mcp_ref) = parse_foreign_mcp_token(raw, dialect) else {
                        return entry.clone();
                    };
                    changed = true;
                    Value::String(mcp_ref.to_canonical())
                })
                .collect();
            (Value::Sequence(lifted), changed)
        }
        other => (other.clone(), false),
    }
}

fn lift_foreign_mcp_tokens_in_field(fm: &mut Frontmatter, key: &str, dialect: Dialect) -> bool {
    let Some(value) = fm.get(key).cloned() else {
        return false;
    };
    let (lifted, changed) = lift_foreign_mcp_tokens_in_value(&value, dialect);
    if changed {
        fm.insert(key, lifted);
    }
    changed
}

fn lift_foreign_tools_allowlist(fm: &mut Frontmatter, dialect: Dialect) -> bool {
    let mut changed = false;
    if fm.get("tools").is_none() {
        for key in ["allowed-tools", "allowed_tools", "tools"] {
            if let Some(value) = fm.remove(key) {
                fm.insert("tools", value);
                changed = true;
                break;
            }
        }
    } else {
        for key in ["allowed-tools", "allowed_tools"] {
            if fm.remove(key).is_some() {
                changed = true;
            }
        }
    }
    changed |= lift_foreign_mcp_tokens_in_field(fm, "tools", dialect);
    changed
}

fn strip_foreign_keys(fm: &mut Frontmatter, keys: &[&str]) -> bool {
    let mut changed = false;
    for key in keys {
        if fm.remove(key).is_some() {
            changed = true;
        }
    }
    changed
}

fn strip_non_canonical_tool_aliases(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    for &(key, _) in tool_policy::NON_CANONICAL_TOOL_FIELD_ALIASES {
        if fm.remove(key).is_some() {
            changed = true;
        }
    }
    changed
}

/// Lift frontmatter, returning whether any field was rewritten.
pub(crate) fn lift_frontmatter_with_change(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> (Frontmatter, bool) {
    let mut lifted = frontmatter.clone();
    let mut changed = matches!(
        item_kind,
        ItemKind::Agent | ItemKind::Skill | ItemKind::BootstrapDoc
    ) && tool_policy::strip_removed_mcp_tools_fields(&mut lifted);
    changed |= match dialect {
        Dialect::MarsNative => strip_non_canonical_tool_aliases(&mut lifted),
        Dialect::Claude => lift_claude(item_kind, &mut lifted),
        Dialect::Codex => lift_codex(item_kind, &mut lifted),
        Dialect::Cursor => lift_cursor(item_kind, &mut lifted),
        Dialect::OpenCode => lift_opencode(item_kind, &mut lifted),
    };
    (lifted, changed)
}

fn lift_claude(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Skill | ItemKind::BootstrapDoc => lift_claude_skill(fm),
        ItemKind::Agent => lift_claude_agent(fm),
        ItemKind::Hook | ItemKind::McpServer => false,
    }
}

fn lift_claude_skill(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if find_invocability_field(fm, "model-invocable").is_none()
        && let Some(disable) = fm.remove("disable-model-invocation")
    {
        changed = true;
        if disable.as_bool() == Some(true) {
            fm.insert("model-invocable", Value::Bool(false));
        }
    } else if fm.remove("disable-model-invocation").is_some() {
        changed = true;
    }

    for removed in ["invocation", "allow_implicit_invocation"] {
        if fm.remove(removed).is_some() {
            changed = true;
        }
    }
    changed |= lift_foreign_tools_allowlist(fm, Dialect::Claude);
    changed
}

fn lift_claude_agent(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if let Some(tools) = fm.remove("disallowedTools") {
        if fm.get("disallowed-tools").is_none() && fm.get("disallowed_tools").is_none() {
            fm.insert("disallowed-tools", tools);
        }
        changed = true;
    }

    changed |= lift_foreign_tools_allowlist(fm, Dialect::Claude);

    if let Some(mcp) = fm.remove("mcpServers") {
        changed |=
            tool_policy::append_mcp_server_entries_to_tools(fm, &tool_policy::yaml_str_list(&mcp));
    }

    changed |= lift_foreign_mcp_tokens_in_field(fm, "disallowed-tools", Dialect::Claude);
    changed
}

fn lift_codex(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    match item_kind {
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            changed |= strip_foreign_keys(
                fm,
                &[
                    "disable-model-invocation",
                    "allow_implicit_invocation",
                    "invocation",
                    "model-invocable",
                    "model_invocable",
                    "user-invocable",
                    "user_invocable",
                ],
            );
            changed |= lift_foreign_tools_allowlist(fm, Dialect::Codex);
        }
        ItemKind::Agent => {
            changed |= strip_foreign_keys(
                fm,
                &[
                    "mode",
                    "tools",
                    "disallowedTools",
                    "disallowed-tools",
                    "tools_denied",
                ],
            );
        }
        ItemKind::Hook | ItemKind::McpServer => return false,
    }
    changed
}

fn lift_cursor(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Skill | ItemKind::BootstrapDoc => lift_cursor_rule(fm),
        ItemKind::Agent => false,
        ItemKind::Hook | ItemKind::McpServer => false,
    }
}

fn lift_cursor_rule(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if find_invocability_field(fm, "model-invocable").is_none()
        && fm.get("alwaysApply").and_then(Value::as_bool) == Some(true)
    {
        fm.insert("model-invocable", Value::Bool(true));
        changed = true;
    }
    if fm.remove("alwaysApply").is_some() {
        changed = true;
    }
    if fm.remove("globs").is_some() {
        changed = true;
    }
    changed |= lift_foreign_tools_allowlist(fm, Dialect::Cursor);
    changed |= lift_foreign_mcp_tokens_in_field(fm, "disallowed-tools", Dialect::Cursor);
    changed
}

fn lift_opencode(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Agent => lift_opencode_agent(fm),
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            let mut changed = false;
            changed |= strip_foreign_keys(fm, &["mode"]);
            changed |= lift_foreign_tools_allowlist(fm, Dialect::OpenCode);
            if let Some(tools) = fm.remove("disallowedTools") {
                if fm.get("disallowed-tools").is_none() && fm.get("disallowed_tools").is_none() {
                    fm.insert("disallowed-tools", tools);
                }
                changed = true;
            }
            if fm.get("disallowed-tools").is_none()
                && let Some(tools) = fm.remove("disallowed_tools")
            {
                fm.insert("disallowed-tools", tools);
                changed = true;
            } else if fm.remove("disallowed_tools").is_some() {
                changed = true;
            }
            changed
        }
        ItemKind::Hook | ItemKind::McpServer => false,
    }
}

fn lift_opencode_agent(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if find_invocability_field(fm, "user-invocable").is_none()
        && fm.get("mode").and_then(Value::as_str) == Some("primary")
    {
        fm.insert("user-invocable", Value::Bool(false));
        changed = true;
    }

    if let Some(tools) = fm.remove("disallowedTools") {
        if fm.get("disallowed-tools").is_none() && fm.get("disallowed_tools").is_none() {
            fm.insert("disallowed-tools", tools);
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::AgentDiagnostic;
    use crate::compiler::agents::parse_agent_content;
    use crate::compiler::lossiness::Lossiness;
    use crate::compiler::skills::lower::{SkillHarness, lower_skill_for_harness};
    use crate::compiler::skills::parse_skill_content;
    use crate::frontmatter::Frontmatter;

    fn fm(yaml: &str) -> Frontmatter {
        Frontmatter::parse(&format!("---\n{yaml}---\n# Body\n")).unwrap()
    }

    fn lift(dialect: Dialect, item_kind: ItemKind, frontmatter: &Frontmatter) -> Frontmatter {
        lift_frontmatter_with_change(dialect, item_kind, frontmatter).0
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
            Some(&Value::Sequence(vec![
                Value::String("mcp(server)".into()),
                Value::String("mcp(other)".into())
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
    fn opencode_primary_mode_sets_user_invocable_false() {
        let lifted = lift(
            Dialect::OpenCode,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\nmode: primary\n"),
        );
        assert_eq!(lifted.get("user-invocable"), Some(&Value::Bool(false)));
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
    fn claude_shaped_skill_stages_and_lowers_end_to_end() {
        let source_yaml = "\
name: demo
description: Demo skill
disable-model-invocation: true
user-invocable: false
allowed-tools: [Bash(git *)]
disallowed-tools: [Agent]
when_to_use: Use when git history matters
";
        let source = format!("---\n{source_yaml}---\n# Body\n");
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Skill,
            &Frontmatter::parse(&source).unwrap(),
        );
        let staged = lifted.render();

        let mut diags = Vec::new();
        let (profile, fm) = parse_skill_content(&staged, &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert!(!profile.model_invocable);
        assert!(!profile.user_invocable);
        assert_eq!(profile.tools, vec!["bash(git *)"]);
        assert_eq!(profile.disallowed_tools, vec!["agent"]);
        assert_eq!(
            profile.when_to_use.as_deref(),
            Some("Use when git history matters")
        );
        assert!(
            !profile
                .passthrough_fields
                .iter()
                .any(|(k, _)| k == "disable-model-invocation")
        );

        let body = fm.body();
        let claude = lower_skill_for_harness(SkillHarness::Claude, &profile, body);
        let claude_out = String::from_utf8(claude.bytes).unwrap();
        assert!(claude_out.contains("disable-model-invocation: true"));
        assert!(claude_out.contains("user-invocable: false"));
        assert!(claude_out.contains("when_to_use:"));
        assert!(claude.lossy_fields.is_empty());

        let codex = lower_skill_for_harness(SkillHarness::Codex, &profile, body);
        assert!(
            codex
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable" && f.classification == Lossiness::Dropped)
        );
        assert!(codex.lossy_fields.iter().any(|f| f.field == "tools"));

        let opencode = lower_skill_for_harness(SkillHarness::OpenCode, &profile, body);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable")
        );

        let cursor = lower_skill_for_harness(SkillHarness::Cursor, &profile, body);
        let cursor_out = String::from_utf8(cursor.bytes).unwrap();
        assert!(cursor.lossy_fields.iter().any(|f| f.field == "tools"));
        assert!(!cursor_out.contains("when_to_use"));
        assert!(!cursor_out.contains("disable-model-invocation"));
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "when_to_use" || f.field == "user-invocable")
        );
    }

    #[test]
    fn claude_skill_allowed_tools_lifts_to_canonical_tools() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Skill,
            &fm("name: s\ndescription: d\nallowed-tools: [Bash(git *)]\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::Sequence(vec![Value::String("Bash(git *)".into())]))
        );
        assert!(lifted.get("allowed-tools").is_none());

        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(profile.tools, vec!["bash(git *)"]);
    }

    #[test]
    fn claude_agent_disallowed_tools_round_trip_through_stage() {
        let source = "---\nname: reviewer\ndescription: Reviews code\ndisallowedTools: [WebSearch]\n---\n# Body\n";
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &Frontmatter::parse(source).unwrap(),
        );
        let staged = lifted.render();
        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(&staged, &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(profile.disallowed_tools, vec!["web_search"]);
    }

    #[test]
    fn claude_agent_lifts_foreign_mcp_refs_in_tool_lists() {
        use crate::compiler::mcp_ref::{parse_mcp_ref, try_parse_mcp_tool_name};

        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\nallowed-tools: [Read, mcp__github__create_issue, mcp__context7__*]\ndisallowedTools: [mcp__github__delete_repo]\n",
            ),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::Sequence(vec![
                Value::String("Read".into()),
                Value::String("mcp(github/create_issue)".into()),
                Value::String("mcp(context7/*)".into()),
            ]))
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String(
                "mcp(github/delete_repo)".into()
            )]))
        );

        for canonical in [
            "mcp(github/create_issue)",
            "mcp(context7/*)",
            "mcp(github/delete_repo)",
        ] {
            let parsed = try_parse_mcp_tool_name(canonical).unwrap();
            let inner = canonical.trim_start_matches("mcp(").trim_end_matches(')');
            assert_eq!(parse_mcp_ref(inner).unwrap(), parsed);
        }

        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools,
            vec!["read", "mcp(github/create_issue)", "mcp(context7/*)"]
        );
        assert_eq!(profile.disallowed_tools, vec!["mcp(github/delete_repo)"]);
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
            Some(&Value::Sequence(vec![
                Value::String("mcp(*/*)".into()),
                Value::String("mcp(github/*)".into()),
            ]))
        );
    }

    #[test]
    fn claude_skill_lifts_foreign_mcp_refs_in_allowed_tools() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Skill,
            &fm(
                "name: s\ndescription: d\nallowed-tools: [mcp__GitHub__CreateIssue, mcp__context7__*]\n",
            ),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::Sequence(vec![
                Value::String("mcp(GitHub/CreateIssue)".into()),
                Value::String("mcp(context7/*)".into()),
            ]))
        );
        assert!(lifted.get("allowed-tools").is_none());

        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools,
            vec!["mcp(GitHub/CreateIssue)", "mcp(context7/*)"]
        );
    }

    #[test]
    fn cursor_rule_lifts_mcp_parenthesis_tokens() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("description: rule\nallowed-tools: [Mcp(github:create_issue), Read]\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::Sequence(vec![
                Value::String("mcp(github/create_issue)".into()),
                Value::String("Read".into()),
            ]))
        );
    }

    #[test]
    fn cursor_rule_lifts_namespaced_server_mcp_token() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("description: rule\nallowed-tools: [Mcp(plugin:context7:context7:create_issue)]\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::Sequence(vec![Value::String(
                "mcp(plugin:context7:context7/create_issue)".into()
            )]))
        );
    }

    #[test]
    fn claude_skill_lifts_scalar_foreign_mcp_token_in_allowed_tools() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Skill,
            &fm("name: s\ndescription: d\nallowed-tools: mcp__github__create_issue\n"),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&Value::String("mcp(github/create_issue)".into()))
        );
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
            Some(&Value::Sequence(vec![
                Value::String("Read".into()),
                Value::String("Bash(git *)".into()),
                Value::String("mcp(context7)".into()),
                Value::String("mcp(github)".into()),
            ]))
        );

        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools,
            vec!["read", "bash(git *)", "mcp(context7)", "mcp(github)"]
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

    #[test]
    fn staging_strips_retired_agent_mcp_tools_field() {
        let input = fm("name: a\ndescription: d\nmcp-tools: [plugin:demo]\n");

        let mut diags = Vec::new();
        let (_, _) = parse_agent_content(&input.render(), &mut diags).unwrap();
        assert!(
            diags.iter().any(
                |d| matches!(d, AgentDiagnostic::RemovedField { field } if field == "mcp-tools")
            ),
            "expected RemovedField diagnostic, got {diags:?}"
        );

        let lifted = lift(Dialect::Claude, ItemKind::Agent, &input);
        assert!(lifted.get("mcp-tools").is_none());
        assert!(lifted.get("mcp_tools").is_none());

        let mut staged_diags = Vec::new();
        let (profile, fm) = parse_agent_content(&lifted.render(), &mut staged_diags).unwrap();
        assert!(
            staged_diags.is_empty(),
            "stripped field must not re-trigger diagnostic: {staged_diags:?}"
        );
        assert!(fm.get("mcp-tools").is_none());
        assert!(profile.tools.is_empty());
    }
}
