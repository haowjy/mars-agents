//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::diagnostic::SchemaDiagnostic;
use crate::dialect::Dialect;
use crate::frontmatter::Frontmatter;
use crate::invocability::find_invocability_field;
use crate::item::ItemKind;
use crate::mcp::{ForeignMcpDialect, parse_foreign_mcp_token};
use crate::tool_names::split_tool_name;
use crate::tool_policy;

/// Rewrite the tool names in `fm[key]` with `rewrite`, which returns the new spelling of a name
/// it changes. The field is a scalar, a list, or a map keyed by tool name (`tools: {mcp__s__t:
/// deny}` carries the name in the KEY, so a rewrite that skipped keys would leak foreign names
/// into canonical `.mars/` unchanged). Map values and key order are preserved.
pub(crate) fn rewrite_tool_entries(
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

pub(crate) fn lift_foreign_mcp_tokens_in_field(
    fm: &mut Frontmatter,
    key: &str,
    dialect: Dialect,
) -> bool {
    let dialect = match dialect {
        Dialect::Claude | Dialect::AgentSkills => Some(ForeignMcpDialect::Claude),
        Dialect::Cursor => Some(ForeignMcpDialect::Cursor),
        Dialect::Codex | Dialect::OpenCode | Dialect::MarsNative => None,
    };
    dialect.is_some_and(|dialect| {
        rewrite_tool_entries(fm, key, |raw| {
            parse_foreign_mcp_token(raw, dialect).map(|mcp_ref| mcp_ref.to_canonical())
        })
    })
}

/// Claude's names for the sub-agent tool (`Task`, now `Agent`). In canonical Mars `task` means
/// the task-list tools, so a Claude-dialect entry must be renamed on the way in.
pub(crate) fn claude_task_to_agent(raw: &str) -> Option<String> {
    let (head, payload) = split_tool_name(raw.trim());
    matches!(head.trim(), "Task" | "Agent").then(|| format!("agent{payload}"))
}

pub(crate) fn lift_claude_task_entries(fm: &mut Frontmatter, key: &str) -> bool {
    rewrite_tool_entries(fm, key, claude_task_to_agent)
}

pub(crate) fn lift_foreign_tools(
    fm: &mut Frontmatter,
    dialect: Dialect,
    native_tools_restrict: bool,
) -> bool {
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

/// A lifted definition: the new frontmatter, whether any field was rewritten, and what the
/// dialect's translation dropped or could not convert.
pub struct Lifted {
    pub frontmatter: Frontmatter,
    pub changed: bool,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

pub fn lift_frontmatter(
    dialect: Dialect,
    item_kind: ItemKind,
    frontmatter: &Frontmatter,
) -> Lifted {
    let mut lifted = frontmatter.clone();
    let mut diagnostics = Vec::new();
    let mut changed = matches!(
        item_kind,
        ItemKind::Agent | ItemKind::Skill | ItemKind::BootstrapDoc
    ) && tool_policy::strip_removed_mcp_tools_fields(&mut lifted);
    changed |= match dialect {
        Dialect::MarsNative => strip_non_canonical_tool_aliases(&mut lifted),
        Dialect::Claude | Dialect::AgentSkills => {
            crate::claude::translate::lift(item_kind, &mut lifted, &mut diagnostics)
        }
        Dialect::Codex => lift_codex(item_kind, &mut lifted),
        Dialect::Cursor => lift_cursor(item_kind, &mut lifted),
        Dialect::OpenCode => lift_opencode(item_kind, &mut lifted),
    };
    Lifted {
        frontmatter: lifted,
        changed,
        diagnostics,
    }
}

fn drop_skill_tool_fields(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    for field in [
        "tools",
        "allowed-tools",
        "allowed_tools",
        "disallowed-tools",
        "disallowed_tools",
        "disallowedTools",
    ] {
        changed |= fm.remove(field).is_some();
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
            changed |= drop_skill_tool_fields(fm);
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

fn cursor_manual_rule_shape(fm: &Frontmatter) -> bool {
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
    changed |= drop_skill_tool_fields(fm);
    changed
}

fn lift_opencode(item_kind: ItemKind, fm: &mut Frontmatter) -> bool {
    match item_kind {
        ItemKind::Agent => lift_opencode_agent(fm),
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            let mut changed = false;
            changed |= strip_foreign_keys(fm, &["mode"]);
            changed |= drop_skill_tool_fields(fm);
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
