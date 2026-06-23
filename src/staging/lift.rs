//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::compiler::invocability::find_invocability_field;
use crate::dialect::Dialect;
use crate::frontmatter::Frontmatter;
use crate::lock::ItemKind;

fn lift_foreign_tools_allowlist(fm: &mut Frontmatter) -> bool {
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

/// Non-canonical tool spellings that must not appear in the canonical store.
const NON_CANONICAL_TOOL_ALIASES: &[&str] = &["allowed-tools", "allowed_tools", "disallowed_tools"];

fn strip_non_canonical_tool_aliases(fm: &mut Frontmatter) -> bool {
    strip_foreign_keys(fm, NON_CANONICAL_TOOL_ALIASES)
}

/// Lift frontmatter, returning whether any field was rewritten.
pub(crate) fn lift_frontmatter_with_change(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> (Frontmatter, bool) {
    let mut lifted = frontmatter.clone();
    let changed = match dialect {
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
    changed |= lift_foreign_tools_allowlist(fm);
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

    if let Some(mcp) = fm.remove("mcpServers") {
        if fm.get("mcp-tools").is_none() {
            fm.insert("mcp-tools", mcp);
        }
        changed = true;
    }
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
            changed |= lift_foreign_tools_allowlist(fm);
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
    changed |= lift_foreign_tools_allowlist(fm);
    changed
}

fn lift_opencode(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Agent => lift_opencode_agent(fm),
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            let mut changed = false;
            changed |= strip_foreign_keys(fm, &["mode"]);
            changed |= lift_foreign_tools_allowlist(fm);
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
    use crate::compiler::skills::lower::{lower_skill_for_harness, SkillHarness};
    use crate::compiler::skills::parse_skill_content;
    use crate::compiler::agents::parse_agent_content;
    use crate::compiler::lossiness::Lossiness;
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
        assert_eq!(
            lifted.get("model-invocable"),
            Some(&Value::Bool(false))
        );
        assert!(lifted.get("disable-model-invocation").is_none());
    }

    #[test]
    fn claude_skill_idempotent_on_existing_model_invocable() {
        let input = fm("name: s\ndescription: d\nmodel-invocable: false\n");
        let lifted = lift(Dialect::Claude, ItemKind::Skill, &input);
        assert!(!lifted.get("model-invocable").is_none());
        assert!(lifted.get("disable-model-invocation").is_none());
    }

    #[test]
    fn claude_agent_removes_duplicate_foreign_keys_when_canonical_present() {
        let lifted = lift(
            Dialect::Claude,
            ItemKind::Agent,
            &fm(
                "name: a\ndescription: d\ndisallowed-tools: [Read]\ndisallowedTools: [Agent]\nmcp-tools: [server]\nmcpServers: [other]\n",
            ),
        );
        assert_eq!(
            lifted.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("Read".into())]))
        );
        assert!(lifted.get("disallowedTools").is_none());
        assert_eq!(
            lifted.get("mcp-tools"),
            Some(&Value::Sequence(vec![Value::String("server".into())]))
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
        assert_eq!(
            lifted.get("user-invocable"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn cursor_always_apply_lifts_model_invocable() {
        let lifted = lift(
            Dialect::Cursor,
            ItemKind::Skill,
            &fm("description: rule\nalwaysApply: true\nglobs: \"**/*\"\n"),
        );
        assert_eq!(
            lifted.get("model-invocable"),
            Some(&Value::Bool(true))
        );
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
        assert!(
            codex
                .lossy_fields
                .iter()
                .any(|f| f.field == "tools")
        );

        let opencode = lower_skill_for_harness(SkillHarness::OpenCode, &profile, body);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable")
        );

        let cursor = lower_skill_for_harness(SkillHarness::Cursor, &profile, body);
        let cursor_out = String::from_utf8(cursor.bytes).unwrap();
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "tools")
        );
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
        let (profile, _) =
            parse_skill_content(&lifted.render(), &mut diags).unwrap();
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
}
