//! Per-dialect foreign → canonical frontmatter lift tables (Phase B3).

use serde_yaml::Value;

use crate::diagnostic::SchemaDiagnostic;
use crate::dialect::Dialect;
use crate::frontmatter::Frontmatter;
use crate::harness::HarnessId;
use crate::harness_overrides::move_harness_only_keys;
use crate::invocability::find_invocability_field;
use crate::item::DefinitionKind;
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
    changed |= lift_tool_flags(fm);
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

/// A lifted definition: the new frontmatter, whether any field was rewritten, what the
/// translation dropped or moved (`changes`), and what it found it could not convert
/// (`findings`).
pub struct Lifted {
    pub frontmatter: Frontmatter,
    pub changed: bool,
    pub changes: Vec<SchemaDiagnostic>,
    pub findings: Vec<SchemaDiagnostic>,
}

pub fn lift_frontmatter(
    dialect: Dialect,
    kind: DefinitionKind,
    frontmatter: &Frontmatter,
) -> Lifted {
    let mut lifted = frontmatter.clone();
    let (mut changes, mut findings) = (Vec::new(), Vec::new());
    // What only pre-v1 Mars wrote is read as such in every dialect, so one such key never
    // changes how its neighbours are read.
    let mut changed = dialect != Dialect::MarsNative
        && crate::upgrade::lift_mars_only_keys(&mut lifted, kind, &mut changes);
    changed |= match dialect {
        Dialect::MarsNative => false,
        Dialect::Claude | Dialect::AgentSkills => {
            crate::claude::translate::lift(kind, &mut lifted, &mut changes, &mut findings)
        }
        Dialect::Codex => lift_codex(kind, &mut lifted),
        Dialect::Cursor => lift_cursor(kind, &mut lifted),
        Dialect::OpenCode => lift_opencode(kind, &mut lifted, &mut changes),
    };
    // Whatever the dialect's rules left that the strict reader rejects is set aside, never left
    // to fail a package its consumer cannot edit.
    if dialect != Dialect::MarsNative {
        changed |= crate::set_aside::set_aside_rejected_keys(
            &mut lifted,
            kind,
            crate::set_aside::Destination::for_dialect(dialect),
            &mut changes,
            &mut findings,
        );
    }
    Lifted {
        frontmatter: lifted,
        changed,
        changes,
        findings,
    }
}

fn drop_skill_tool_fields(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    for key in crate::skill::tool_keys() {
        changed |= fm.remove(key).is_some();
    }
    changed
}

fn lift_codex(kind: DefinitionKind, fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    match kind {
        DefinitionKind::Skill => {
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
        DefinitionKind::Agent => {
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
    }
    changed
}

fn lift_cursor(kind: DefinitionKind, fm: &mut Frontmatter) -> bool {
    match kind {
        DefinitionKind::Skill => lift_cursor_rule(fm),
        DefinitionKind::Agent => false,
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

fn lift_opencode(
    kind: DefinitionKind,
    fm: &mut Frontmatter,
    changes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    match kind {
        DefinitionKind::Agent => lift_opencode_agent(fm, changes),
        DefinitionKind::Skill => {
            let mut changed = false;
            changed |= strip_foreign_keys(fm, &["mode"]);
            changed |= drop_skill_tool_fields(fm);
            changed
        }
    }
}

/// Agent keys OpenCode reads and schema v1 does not define; they move to
/// `harness-overrides.opencode`, which only `.opencode/` output receives.
const OPENCODE_ONLY_AGENT_KEYS: &[&str] = &[
    "permission",
    "temperature",
    "top_p",
    "steps",
    "maxSteps",
    "options",
    "hidden",
    "disable",
    "color",
    "prompt",
];

/// Whether an Agent is written the way only OpenCode writes one: a `permission` map, or a `tools`
/// map of booleans. Prediction reads this; `lift_opencode_agent` translates the same two spellings.
pub(crate) fn is_opencode_agent(fm: &Frontmatter) -> bool {
    matches!(fm.get("permission"), Some(Value::Mapping(_))) || tool_flags(fm).is_some()
}

fn lift_opencode_agent(fm: &mut Frontmatter, changes: &mut Vec<SchemaDiagnostic>) -> bool {
    let mut changed = false;

    let mut denied = Vec::new();
    for key in ["disallowedTools", "disallowed-tools", "disallowed_tools"] {
        if let Some(value) = fm.remove(key) {
            changed = true;
            denied.extend(tool_policy::tool_entries(&value));
        }
    }
    changed |= lift_tool_flags(fm);
    if !denied.is_empty() {
        let mut tools = match fm.get("tools") {
            Some(Value::Mapping(existing)) => existing.clone(),
            _ => serde_yaml::Mapping::new(),
        };
        for name in denied {
            tools.insert(Value::String(name), Value::String("deny".into()));
        }
        fm.insert("tools", Value::Mapping(tools));
    }
    changed |= move_harness_only_keys(
        fm,
        DefinitionKind::Agent,
        HarnessId::OpenCode,
        OPENCODE_ONLY_AGENT_KEYS,
        changes,
    );
    changed
}

/// The `tools` map when it holds a boolean: OpenCode's form, which no other dialect writes.
fn tool_flags(fm: &Frontmatter) -> Option<&serde_yaml::Mapping> {
    match fm.get("tools") {
        Some(Value::Mapping(tools)) if tools.values().any(Value::is_bool) => Some(tools),
        _ => None,
    }
}

/// A foreign `tools` map of tool name to boolean (OpenCode's form) becomes schema v1's map of tool
/// name to `allow` or `deny`.
fn lift_tool_flags(fm: &mut Frontmatter) -> bool {
    let Some(tools) = tool_flags(fm) else {
        return false;
    };
    let policy = tools
        .iter()
        .map(|(name, flag)| {
            let action = match flag {
                Value::Bool(true) => Value::String("allow".into()),
                Value::Bool(false) => Value::String("deny".into()),
                other => other.clone(),
            };
            (name.clone(), action)
        })
        .collect();
    fm.insert("tools", Value::Mapping(policy));
    true
}
