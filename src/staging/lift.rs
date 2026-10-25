//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::compiler::mcp_ref::parse_foreign_mcp_token;
use crate::dialect::Dialect;
use mars_schema::frontmatter::Frontmatter;
use mars_schema::invocability::find_invocability_field;
use mars_schema::item::ItemKind;
use mars_schema::tool_names::split_tool_name;
use mars_schema::tool_policy;

/// Rewrite the tool names in `fm[key]` with `rewrite`, which returns the new spelling of a name
/// it changes. The field is a scalar, a list, or a map keyed by tool name (`tools: {mcp__s__t:
/// deny}` carries the name in the KEY, so a rewrite that skipped keys would leak foreign names
/// into canonical `.mars/` unchanged). Map values and key order are preserved.
fn rewrite_tool_entries(
    fm: &mut Frontmatter,
    key: &str,
    rewrite: impl Fn(&str) -> Option<String>,
) -> bool {
    let Some(value) = fm.get(key) else {
        return false;
    };
    let mut changed = false;
    let mut swap = |entry: &Value| match entry.as_str().and_then(&rewrite) {
        Some(renamed) => {
            changed = true;
            Value::String(renamed)
        }
        None => entry.clone(),
    };
    let lifted = match value {
        Value::String(_) => swap(value),
        Value::Sequence(seq) => Value::Sequence(seq.iter().map(&mut swap).collect()),
        Value::Mapping(map) => Value::Mapping(
            map.iter()
                .map(|(name, entry)| (swap(name), entry.clone()))
                .collect(),
        ),
        _ => return false,
    };
    if changed {
        fm.insert(key, lifted);
    }
    changed
}

/// A comma-separated scalar (`tools: Read, Task`, Claude's own agent-file spelling) is a list.
/// Split it first so the per-entry lifts below see each entry, not one string.
fn split_scalar_tool_list(fm: &mut Frontmatter, key: &str) -> bool {
    let Some(value @ Value::String(_)) = fm.get(key) else {
        return false;
    };
    let entries = tool_policy::tool_entries(value);
    if entries.len() < 2 {
        return false;
    }
    fm.insert(
        key,
        Value::Sequence(entries.into_iter().map(Value::String).collect()),
    );
    true
}

/// Rewrite a foreign scalar/list of tool names as canonical policy entries. Foreign formats use
/// list-shaped fields for two different meanings: Claude agent `tools:` restricts the agent to
/// the list, while skill `allowed-tools` and the other skill dialects only grant their entries.
/// Map-form input is already a canonical policy and must keep its authored actions.
fn lift_tool_list_to_policy_map(fm: &mut Frontmatter, restricts: bool) -> bool {
    let Some(value) = fm.get("tools") else {
        return false;
    };
    let entries = match value {
        Value::String(_) => tool_policy::tool_entries(value),
        Value::Sequence(entries) if entries.iter().all(|entry| entry.as_str().is_some()) => {
            tool_policy::yaml_str_list(value)
        }
        Value::Sequence(_)
        | Value::Mapping(_)
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::Tagged(_) => return false,
    };

    let mut policy = serde_yaml::Mapping::new();
    if restricts {
        policy.insert(Value::String("*".into()), Value::String("deny".into()));
    }
    for entry in entries {
        policy.insert(Value::String(entry), Value::String("allow".into()));
    }
    fm.insert("tools", Value::Mapping(policy));
    true
}

fn lift_foreign_mcp_tokens_in_field(fm: &mut Frontmatter, key: &str, dialect: Dialect) -> bool {
    rewrite_tool_entries(fm, key, |raw| {
        parse_foreign_mcp_token(raw, dialect).map(|mcp_ref| mcp_ref.to_canonical())
    })
}

/// Claude's pre-`Agent` name for the sub-agent tool. In canonical Mars `task` means the
/// task-list tools, so a Claude-dialect `Task` entry must be renamed on the way in.
fn claude_task_to_agent(raw: &str) -> Option<String> {
    let (head, payload) = split_tool_name(raw.trim());
    (head.trim() == "Task").then(|| format!("agent{payload}"))
}

fn lift_claude_task_entries(fm: &mut Frontmatter, key: &str) -> bool {
    rewrite_tool_entries(fm, key, claude_task_to_agent)
}

fn lift_foreign_tools(fm: &mut Frontmatter, dialect: Dialect, native_tools_restrict: bool) -> bool {
    let mut changed = false;
    let mut came_from_native_tools = fm.get("tools").is_some();
    if fm.get("tools").is_none() {
        for key in ["allowed-tools", "allowed_tools"] {
            if let Some(value) = fm.remove(key) {
                fm.insert("tools", value);
                changed = true;
                came_from_native_tools = false;
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
    changed |= lift_tool_list_to_policy_map(fm, native_tools_restrict && came_from_native_tools);
    changed |= lift_foreign_mcp_tokens_in_field(fm, "tools", dialect);
    if dialect == Dialect::Claude {
        changed |= lift_claude_task_entries(fm, "tools");
    }
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
    changed |= lift_foreign_tools(fm, Dialect::Claude, false);
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

    changed |= lift_foreign_tools(fm, Dialect::Claude, true);

    if let Some(mcp) = fm.remove("mcpServers") {
        changed |=
            tool_policy::append_mcp_server_entries_to_tools(fm, &tool_policy::yaml_str_list(&mcp));
    }

    changed |= split_scalar_tool_list(fm, "disallowed-tools");
    changed |= lift_foreign_mcp_tokens_in_field(fm, "disallowed-tools", Dialect::Claude);
    changed |= lift_claude_task_entries(fm, "disallowed-tools");
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
            changed |= lift_foreign_tools(fm, Dialect::Codex, false);
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

pub(crate) fn cursor_manual_rule_shape(fm: &Frontmatter) -> bool {
    fm.get("alwaysApply").and_then(Value::as_bool) == Some(false)
        && fm.get("description").is_none()
        && fm.get("globs").is_none()
}

fn lift_cursor_rule(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if find_invocability_field(fm, "model-invocable").is_none() {
        match fm.get("alwaysApply").and_then(Value::as_bool) {
            Some(true) => {
                fm.insert("model-invocable", Value::Bool(true));
                changed = true;
            }
            Some(false) if cursor_manual_rule_shape(fm) => {
                fm.insert("model-invocable", Value::Bool(false));
                changed = true;
            }
            _ => {}
        }
    }
    if fm.remove("alwaysApply").is_some() {
        changed = true;
    }
    if fm.remove("globs").is_some() {
        changed = true;
    }
    changed |= lift_foreign_tools(fm, Dialect::Cursor, false);
    changed |= lift_foreign_mcp_tokens_in_field(fm, "disallowed-tools", Dialect::Cursor);
    changed
}

fn lift_opencode(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Agent => lift_opencode_agent(fm),
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            let mut changed = false;
            changed |= strip_foreign_keys(fm, &["mode"]);
            changed |= lift_foreign_tools(fm, Dialect::OpenCode, false);
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
    use crate::compiler::agents::lower::{NativeModel, lower_to_claude};
    use crate::compiler::agents::parse_agent_content_legacy;
    use crate::compiler::lossiness::Lossiness;
    use crate::compiler::skills::lower::{SkillHarness, lower_skill_for_harness};
    use crate::compiler::skills::parse_skill_content_legacy;
    use mars_schema::frontmatter::Frontmatter;

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
        let (profile, _) = parse_skill_content_legacy(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
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
        let (profile, fm) = parse_skill_content_legacy(&staged, &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert!(!profile.model_invocable);
        assert!(!profile.user_invocable);
        assert_eq!(
            profile.tools.describe(),
            ["allow:bash(git *)", "deny:agent"]
        );
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
            !codex
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable" && f.classification == Lossiness::Dropped)
        );
        assert!(codex.lossy_fields.iter().any(|f| f.field == "tools"));
        assert_eq!(codex.siblings.len(), 1);

        let opencode = lower_skill_for_harness(SkillHarness::OpenCode, &profile, body);
        // OpenCode has no model-invocation gate: the dropped axis is an accepted
        // known limitation and must not surface as lossiness. Tool gating is a
        // genuine loss and still warns.
        assert!(
            !opencode
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable" && f.classification == Lossiness::Dropped)
        );
        assert!(opencode.lossy_fields.iter().any(|f| f.field == "tools"));

        let cursor = lower_skill_for_harness(SkillHarness::Cursor, &profile, body);
        let cursor_out = String::from_utf8(cursor.bytes).unwrap();
        assert!(cursor.lossy_fields.iter().any(|f| f.field == "tools"));
        assert!(!cursor_out.contains("when_to_use"));
        assert!(!cursor_out.contains("disable-model-invocation"));
        assert!(!cursor_out.contains("description:"));
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "user-invocable")
        );
        assert!(
            !cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "when_to_use" || f.field == "model-invocable")
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
            Some(&tool_map(&[("Bash(git *)", "allow")]))
        );
        assert!(lifted.get("allowed-tools").is_none());

        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content_legacy(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(profile.tools.describe(), ["allow:bash(git *)"]);
    }

    #[test]
    fn codex_tool_fields_follow_the_existing_skill_and_agent_meanings() {
        let skill = lift(
            Dialect::Codex,
            ItemKind::Skill,
            &fm("name: s\ndescription: d\nallowed-tools: [Read]\n"),
        );
        assert_eq!(skill.get("tools"), Some(&tool_map(&[("Read", "allow")])));
        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content_legacy(&skill.render(), &mut diags).unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(profile.tools.describe(), ["allow:read"]);
        assert!(!profile.tools.is_allowlist());

        let agent = lift(
            Dialect::Codex,
            ItemKind::Agent,
            &fm("name: a\ndescription: d\ntools: [Read]\n"),
        );
        assert!(
            agent.get("tools").is_none(),
            "Codex native agents have no tool-policy field"
        );
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
        let (profile, _) = parse_agent_content_legacy(&staged, &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(profile.tools.describe(), ["deny:web_search"]);
    }

    #[test]
    fn claude_agent_lifts_foreign_mcp_refs_in_tool_lists() {
        use mars_schema::mcp::{parse_mcp_ref, try_parse_mcp_tool_name};

        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\nallowed-tools: [Read, mcp__github__create_issue, mcp__context7__*]\ndisallowedTools: [mcp__github__delete_repo]\n",
            ),
        );
        assert_eq!(
            lifted.get("tools"),
            Some(&tool_map(&[
                ("Read", "allow"),
                ("mcp(github/create_issue)", "allow"),
                ("mcp(context7/*)", "allow"),
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
        let (profile, _) = parse_agent_content_legacy(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools.describe(),
            [
                "allow:read",
                "allow:mcp(github/create_issue)",
                "allow:mcp(context7/*)",
                "deny:mcp(github/delete_repo)"
            ]
        );
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
        let (profile, canonical) =
            parse_agent_content_legacy(&lifted.render(), &mut diags).unwrap();
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
            Some(&tool_map(&[
                ("mcp(GitHub/CreateIssue)", "allow"),
                ("mcp(context7/*)", "allow"),
            ]))
        );
        assert!(lifted.get("allowed-tools").is_none());

        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content_legacy(&lifted.render(), &mut diags).unwrap();
        assert!(diags.is_empty());
        assert_eq!(
            profile.tools.describe(),
            ["allow:mcp(GitHub/CreateIssue)", "allow:mcp(context7/*)"]
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
    fn cursor_lifts_foreign_mcp_token_in_map_form_tools_key() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("description: rule\ntools:\n  Mcp(github:delete_repo): deny\n"),
        );
        let mut expected = serde_yaml::Mapping::new();
        expected.insert(
            Value::String("mcp(github/delete_repo)".into()),
            Value::String("deny".into()),
        );
        assert_eq!(lifted.get("tools"), Some(&Value::Mapping(expected)));
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
            Some(&tool_map(&[
                ("mcp(github/create_issue)", "allow"),
                ("Read", "allow"),
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
            Some(&tool_map(&[(
                "mcp(plugin:context7:context7/create_issue)",
                "allow",
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
            Some(&tool_map(&[("mcp(github/create_issue)", "allow")]))
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
            Some(&tool_map(&[
                ("Read", "allow"),
                ("Bash(git *)", "allow"),
                ("mcp(context7)", "allow"),
                ("mcp(github)", "allow"),
            ]))
        );

        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content_legacy(&lifted.render(), &mut diags).unwrap();
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
