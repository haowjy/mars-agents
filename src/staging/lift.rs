//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::compiler::invocability::find_invocability_field;
use crate::dialect::Dialect;
use crate::frontmatter::Frontmatter;
use crate::lock::ItemKind;

/// Lift foreign frontmatter to canonical mars fields for the resolved dialect.
pub fn lift_frontmatter(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> Frontmatter {
    lift_frontmatter_with_change(dialect, item_kind, frontmatter).0
}

/// Lift frontmatter, returning whether any field was rewritten.
pub fn lift_frontmatter_with_change(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> (Frontmatter, bool) {
    if dialect == Dialect::MarsNative {
        return (frontmatter.clone(), false);
    }

    let mut lifted = frontmatter.clone();
    let changed = match dialect {
        Dialect::MarsNative => false,
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
    changed
}

fn lift_claude_agent(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    if fm.get("disallowed-tools").is_none() && fm.get("disallowed_tools").is_none()
        && let Some(tools) = fm.remove("disallowedTools")
    {
        fm.insert("disallowed-tools", tools);
        changed = true;
    }

    if fm.get("mcp-tools").is_none() && let Some(mcp) = fm.remove("mcpServers") {
        fm.insert("mcp-tools", mcp);
        changed = true;
    }
    changed
}

fn lift_codex(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    let keys = match item_kind {
        ItemKind::Skill | ItemKind::BootstrapDoc => &[
            "disable-model-invocation",
            "allow_implicit_invocation",
            "invocation",
            "model-invocable",
            "model_invocable",
            "user-invocable",
            "user_invocable",
            "allowed-tools",
            "allowed_tools",
            "disallowed-tools",
            "disallowed_tools",
        ][..],
        ItemKind::Agent => &[
            "mode",
            "tools",
            "disallowedTools",
            "disallowed-tools",
            "tools_denied",
        ][..],
        ItemKind::Hook | ItemKind::McpServer => return false,
    };
    let mut changed = false;
    for key in keys {
        if fm.remove(key).is_some() {
            changed = true;
        }
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
    changed
}

fn lift_opencode(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Agent => lift_opencode_agent(fm),
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            let mut changed = false;
            for key in ["mode", "tools", "disallowedTools", "disallowed-tools"] {
                if fm.remove(key).is_some() {
                    changed = true;
                }
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

    if fm.get("disallowed-tools").is_none() && fm.get("disallowed_tools").is_none()
        && let Some(tools) = fm.remove("disallowedTools")
    {
        fm.insert("disallowed-tools", tools);
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

    #[test]
    fn claude_skill_disable_model_invocation_inverts_to_model_invocable() {
        let lifted = lift_frontmatter(
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
        let lifted = lift_frontmatter(Dialect::Claude, ItemKind::Skill, &input);
        assert_eq!(lifted.render(), input.render());
    }

    #[test]
    fn claude_agent_disallowed_tools_camel_case() {
        let lifted = lift_frontmatter(
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
        let lifted = lift_frontmatter(
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
        let lifted = lift_frontmatter(
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
        let lifted = lift_frontmatter(
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
        assert_eq!(profile.allowed_tools, vec!["bash(git *)"]);
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
                .any(|f| f.field == "allowed-tools")
        );

        let opencode = lower_skill_for_harness(SkillHarness::OpenCode, &profile, body);
        assert!(
            opencode
                .lossy_fields
                .iter()
                .any(|f| f.field == "model-invocable")
        );

        let cursor = lower_skill_for_harness(SkillHarness::Cursor, &profile, body);
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "allowed-tools")
        );
        assert!(
            cursor
                .lossy_fields
                .iter()
                .any(|f| f.field == "when_to_use" || f.field == "user-invocable")
                || !String::from_utf8(cursor.bytes).unwrap().contains("when_to_use")
        );
    }

    #[test]
    fn claude_agent_disallowed_tools_round_trip_through_stage() {
        let source = "---\nname: reviewer\ndescription: Reviews code\ndisallowedTools: [WebSearch]\n---\n# Body\n";
        let lifted = lift_frontmatter(
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
