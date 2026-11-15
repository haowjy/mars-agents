//! Claude frontmatter into schema v1: the skills and Agents tables of the import contract.
//! Bodies and payload are never touched.

use serde_yaml::{Mapping, Value};

use crate::HarnessId;
use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
use crate::frontmatter::Frontmatter;
use crate::harness_overrides::move_harness_only_keys;
use crate::item::DefinitionKind;
use crate::lift::{ClaudeTool, claude_tool, lift_foreign_tools};
use crate::mcp::{ForeignMcpDialect, parse_foreign_mcp_token};
use crate::tool_names::{Capability, parse_mars_tool_name};
use crate::tool_natives;
use crate::tool_policy;

/// Skill keys Claude reads and schema v1 does not define. They move to
/// `harness-overrides.claude`, which only `.claude/` output receives. (`model` and `effort`
/// have v1 meaning on Agents only; a skill carries neither.) `tools` has its own warning.
const CLAUDE_ONLY_SKILL_KEYS: &[&str] = &[
    "argument-hint",
    "arguments",
    "model",
    "effort",
    "context",
    "agent",
    "hooks",
    "paths",
    "shell",
    "allowed-tools",
    "disallowed-tools",
    "disallowedTools",
];

/// Spellings only Claude writes, which a manifest-free tree can be predicted Claude from.
/// Deliberately narrower than the vocabulary above and the rewrites below: `model`, `effort`,
/// `tools` and `hooks` also appear in Mars's own files, and the rewritten spellings are read
/// in code, not in a table, so this list is not derived from either. `allowed-tools` is not
/// here: the Agent Skills standard defines it, so a skill carrying it is not Claude's.
pub(crate) const EVIDENCE_KEYS: &[&str] = &[
    "disable-model-invocation",
    "disallowedTools",
    "disallowed-tools",
    "argument-hint",
    "mcpServers",
];

const CLAUDE_ONLY_AGENT_KEYS: &[&str] = &[
    "color",
    "initialPrompt",
    "maxTurns",
    "permissionMode",
    "memory",
    "background",
    "isolation",
    "hooks",
    "autoCompactWindow",
    "omitClaudeMd",
    "experimental",
];

/// `changes` gets what the lift dropped or moved; `findings` what it could not convert.
pub(crate) fn lift(
    kind: DefinitionKind,
    fm: &mut Frontmatter,
    changes: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) -> bool {
    match kind {
        DefinitionKind::Skill => lift_skill(fm, changes, findings),
        DefinitionKind::Agent => lift_agent(fm, changes, findings),
    }
}

fn warn(diagnostics: &mut Vec<SchemaDiagnostic>, code: &str, message: String) {
    diagnostics.push(SchemaDiagnostic::new(
        code,
        Severity::Warning,
        Vec::new(),
        message,
    ));
}

fn info(diagnostics: &mut Vec<SchemaDiagnostic>, code: &str, message: String) {
    diagnostics.push(SchemaDiagnostic::new(
        code,
        Severity::Info,
        Vec::new(),
        message,
    ));
}

/// Move the kind's Claude-only keys under `harness-overrides.claude`.
pub(crate) fn move_to_claude_overrides(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    changes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let keys = match kind {
        DefinitionKind::Agent => CLAUDE_ONLY_AGENT_KEYS,
        DefinitionKind::Skill => CLAUDE_ONLY_SKILL_KEYS,
    };
    move_harness_only_keys(fm, kind, HarnessId::Claude, keys, changes)
}

fn remove_keys(
    fm: &mut Frontmatter,
    keys: impl IntoIterator<Item = &'static str>,
) -> Vec<&'static str> {
    keys.into_iter()
        .filter(|key| fm.remove(key).is_some())
        .collect()
}

/// Claude's `disable-model-invocation: true` is `model-invocable: false`. An existing
/// `model-invocable: false` stays; beside `model-invocable: true` the restricting value wins, with
/// a note.
fn fold_disable_model_invocation(fm: &mut Frontmatter, notes: &mut Vec<SchemaDiagnostic>) -> bool {
    let Some(disable) = fm.remove("disable-model-invocation") else {
        return false;
    };
    if disable.as_bool() != Some(true) {
        return true;
    }
    match fm.get("model-invocable") {
        None => fm.insert("model-invocable", Value::Bool(false)),
        Some(Value::Bool(true)) => {
            fm.insert("model-invocable", Value::Bool(false));
            notes.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Warning,
                key_path("disable-model-invocation"),
                format!(
                    "`disable-model-invocation: {}` conflicts with `model-invocable: true`; kept `false`, the value that restricts",
                    crate::ordered::plain(&disable),
                ),
            ));
        }
        Some(_) => {}
    }
    true
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn lift_skill(
    fm: &mut Frontmatter,
    changes: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let mut changed = false;
    let rendered = fm.render();
    if rendered.contains("${CLAUDE_PLUGIN_ROOT}") || rendered.contains("${CLAUDE_SKILL_DIR}") {
        warn(
            findings,
            "skill.plugin-variable",
            "refers to `${CLAUDE_PLUGIN_ROOT}` or `${CLAUDE_SKILL_DIR}`, which are left as written and do not resolve outside Claude Code".into(),
        );
    }

    changed |= fold_disable_model_invocation(fm, changes);

    // `type` and a top-level `version` live under `metadata`, which already holds string pairs.
    if matches!(fm.get("metadata"), None | Some(Value::Mapping(_))) {
        let moved = ["type", "version"]
            .into_iter()
            .filter_map(|key| fm.remove(key).map(|value| (key, scalar_text(&value))))
            .collect::<Vec<_>>();
        if !moved.is_empty() {
            let mut metadata = match fm.get("metadata") {
                Some(Value::Mapping(existing)) => existing.clone(),
                _ => Mapping::new(),
            };
            for (key, text) in moved {
                let key = Value::String(key.to_owned());
                if let Some(text) = text
                    && !metadata.contains_key(&key)
                {
                    metadata.insert(key, Value::String(text));
                }
            }
            fm.insert("metadata", Value::Mapping(metadata));
            changed = true;
        }
    }

    if let Some(extra) = fm.remove("when_to_use") {
        changed = true;
        if let Some(extra) = scalar_text(&extra) {
            let description = match fm.get("description").and_then(Value::as_str) {
                Some(description) => format!("{} {}", description.trim_end(), extra.trim()),
                None => extra.trim().to_owned(),
            };
            fm.insert("description", Value::String(description));
        }
    }

    // `allowed-tools` pre-approves tools while the skill runs and `disallowed-tools` removes them;
    // schema v1 has no skill tool rules, so both stay Claude's (see `CLAUDE_ONLY_SKILL_KEYS`).
    // The other tool keys mean nothing to Claude and go.
    let dropped = remove_keys(
        fm,
        crate::skill::tool_keys().filter(|key| !CLAUDE_ONLY_SKILL_KEYS.contains(key)),
    );
    if !dropped.is_empty() {
        changed = true;
        warn(
            changes,
            "skill.tools-dropped",
            format!(
                "removed {}: schema v1 has no skill tool rules",
                dropped.join(", ")
            ),
        );
    }

    changed |= move_to_claude_overrides(fm, DefinitionKind::Skill, changes);
    changed
}

/// Server names from `mcpServers`: a list of names, a map keyed by name, or a list of
/// one-key definitions. Returns whether any inline definition was seen.
fn mcp_server_names(value: &Value) -> (Vec<String>, bool) {
    let mut names = Vec::new();
    let mut has_definitions = false;
    let mut collect = |entry: &Value| match entry {
        Value::String(name) => names.push(name.clone()),
        Value::Mapping(map) => {
            has_definitions = true;
            names.extend(map.keys().filter_map(|key| key.as_str().map(str::to_owned)));
        }
        _ => {}
    };
    match value {
        Value::Sequence(entries) => entries.iter().for_each(&mut collect),
        other => collect(other),
    }
    (names, has_definitions)
}

/// Bring the entries of `tools` to the names Claude uses today, and say what that does to what
/// the agent can use.
///
/// A name Claude renamed becomes its new name; a retired one is removed (Claude ignores it too).
/// Mars reads each remaining name as a capability, and Claude lowers a capability to all of its
/// tools, so a grant can reach more than the author listed (`Edit` also enables `Write` and
/// `NotebookEdit`). That widening is a finding: the list was a restriction.
fn rename_claude_tools(
    fm: &mut Frontmatter,
    changes: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let Some(Value::Mapping(tools)) = fm.get("tools") else {
        return false;
    };
    let mut renamed = Mapping::new();
    let mut retired = Vec::new();
    let mut changed = false;
    for (key, action) in tools {
        let name = match key.as_str().map(claude_tool) {
            Some(ClaudeTool::Renamed(name)) => name,
            Some(ClaudeTool::Retired) => {
                retired.extend(key.as_str().map(str::to_owned));
                changed = true;
                continue;
            }
            Some(ClaudeTool::Current) | None => {
                renamed.insert(key.clone(), action.clone());
                continue;
            }
        };
        changed = true;
        // Two entries meeting under one name keep the stricter action.
        let stricter = |kept: &Value, new: &Value| match (kept.as_str(), new.as_str()) {
            (Some("deny"), _) => kept.clone(),
            (_, Some("deny")) => new.clone(),
            (Some("ask"), _) => kept.clone(),
            (_, Some("ask")) => new.clone(),
            _ => kept.clone(),
        };
        let key = Value::String(name);
        let action = match renamed.get(&key) {
            Some(kept) => stricter(kept, action),
            None => action.clone(),
        };
        renamed.insert(key, action);
    }
    if !retired.is_empty() {
        info(
            changes,
            "agent.tools-retired",
            format!(
                "removed {}: Claude no longer has the tool",
                retired
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    widened(&renamed, findings);
    if changed {
        fm.insert("tools", Value::Mapping(renamed));
    }
    changed
}

/// Report what an allowlist grants on Claude beyond the tools it names.
fn widened(tools: &Mapping, findings: &mut Vec<SchemaDiagnostic>) {
    if tools.get("*").and_then(Value::as_str) != Some("deny") {
        return;
    }
    let granted: Vec<&str> = tools
        .iter()
        .filter(|(_, action)| action.as_str() == Some("allow"))
        .filter_map(|(key, _)| key.as_str())
        .collect();
    let mut extra = Vec::new();
    for name in &granted {
        let Ok(parsed) = parse_mars_tool_name(name) else {
            continue;
        };
        if parsed.pattern.is_some() {
            continue;
        }
        let Capability::Builtin(capability) = parsed.capability else {
            continue;
        };
        let Some(row) = tool_natives::lookup(capability, HarnessId::Claude) else {
            continue;
        };
        for tool in row.native_tools() {
            if !granted.iter().any(|name| name.eq_ignore_ascii_case(tool))
                && !matches!(claude_tool(tool), ClaudeTool::Retired)
                && !extra.contains(tool)
            {
                extra.push(*tool);
            }
        }
    }
    if !extra.is_empty() {
        warn(
            findings,
            "agent.tools-widened",
            format!(
                "Mars reads each tool as a capability, so Claude also gets {}, which the list did not name",
                extra.join(", ")
            ),
        );
    }
}

fn lift_agent(
    fm: &mut Frontmatter,
    changes: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let mut changed = false;
    let mut denied = Vec::new();
    for key in tool_policy::DISALLOWED_SPELLINGS {
        if let Some(value) = fm.remove(key) {
            changed = true;
            denied.extend(tool_policy::tool_entries(&value));
        }
    }

    changed |= lift_foreign_tools(fm, crate::dialect::Dialect::Claude, true);

    if let Some(servers) = fm.remove("mcpServers") {
        changed = true;
        let (names, has_definitions) = mcp_server_names(&servers);
        tool_policy::append_mcp_server_entries_to_tools(fm, &names);
        if has_definitions {
            warn(
                findings,
                "agent.mcp-not-converted",
                "inline `mcpServers` definitions are not converted; only the server names carry over".into(),
            );
        }
    }

    if !denied.is_empty() {
        let mut tools = match fm.get("tools") {
            Some(Value::Mapping(existing)) => existing.clone(),
            _ => Mapping::new(),
        };
        for entry in denied {
            let name = parse_foreign_mcp_token(&entry, ForeignMcpDialect::Claude)
                .map(|mcp_ref| mcp_ref.to_canonical())
                .unwrap_or(entry);
            tools.insert(Value::String(name), Value::String("deny".into()));
        }
        fm.insert("tools", Value::Mapping(tools));
    }
    changed |= rename_claude_tools(fm, changes, findings);

    if fm
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.trim().eq_ignore_ascii_case("inherit"))
    {
        fm.remove("model");
        changed = true;
        info(
            changes,
            "agent.model-inherit",
            "removed `model: inherit`: a Claude agent with no `model` runs on the model of the session that starts it".into(),
        );
    }

    changed |= move_to_claude_overrides(fm, DefinitionKind::Agent, changes);
    changed
}

/// Quote the unquoted top-level `key: value` lines that YAML cannot read as plain scalars,
/// using the rule Claude Code applies when its own frontmatter parse fails: a key of letters,
/// `_` or `-`, and a value containing any of `{}[]*&#!|>%@`, a backtick, or `: `. A value
/// already wholly quoted, or a `[...]` that parses as a list, is left alone. Mars is more
/// lenient in three ways: it also quotes a value ending in `:`, folds the indented
/// continuation lines of a value it quotes, and leaves a block scalar header (`|`, `>-`) and
/// CRLF line ends alone. Returns the repaired document and the keys it quoted, or `None` when
/// nothing needed quoting.
pub(crate) fn repair_plain_values(text: &str) -> Option<(String, Vec<String>)> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return None;
    }
    let close = lines
        .iter()
        .skip(1)
        .position(|line| line.trim_end() == "---")?
        + 1;
    let mut repaired = String::with_capacity(text.len() + 16);
    let mut quoted_keys = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let entry = (index <= close && index > 1)
            .then(|| plain_entry(line))
            .flatten();
        let Some((key, value)) = entry else {
            repaired.push_str(line);
            continue;
        };
        // Continuation lines of a plain scalar are indented and fold into one line.
        let start = index - 1;
        let mut folded = value.to_owned();
        while index < close
            && lines[index].starts_with([' ', '\t'])
            && !lines[index].trim().is_empty()
        {
            folded.push(' ');
            folded.push_str(lines[index].trim());
            index += 1;
        }
        // `[Read,` plus an indented `Grep]` is a flow list across lines.
        if is_flow_list(&folded) {
            lines[start..index]
                .iter()
                .for_each(|line| repaired.push_str(line));
            continue;
        }
        repaired.push_str(&format!(
            "{key}: {}\n",
            serde_json::to_string(&folded).ok()?
        ));
        quoted_keys.push(key.to_owned());
    }
    (!quoted_keys.is_empty()).then_some((repaired, quoted_keys))
}

/// A top-level `key: value` line Claude Code would quote.
fn plain_entry(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'-'))
        || !value.starts_with([' ', '\t'])
    {
        return None;
    }
    let value = value.trim();
    let wholly_quoted = value.len() >= 2
        && ['"', '\'']
            .iter()
            .any(|quote| value.starts_with(*quote) && value.ends_with(*quote));
    let block_header = value
        .strip_prefix(['|', '>'])
        .is_some_and(|rest| rest.chars().all(|c| matches!(c, '+' | '-' | '0'..='9')));
    let needs_quotes = value.contains(|c| "{}[]*&#!|>%@`".contains(c))
        || value.contains(": ")
        || value.ends_with(':');
    (!value.is_empty() && needs_quotes && !wholly_quoted && !block_header && !is_flow_list(value))
        .then_some((key, value))
}

fn is_flow_list(value: &str) -> bool {
    value.starts_with('[') && matches!(serde_yaml::from_str::<Value>(value), Ok(Value::Sequence(_)))
}

pub(crate) const REPAIRED: &str = "frontmatter.repaired";

pub(crate) fn repaired_diagnostic(keys: &[String]) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        REPAIRED,
        Severity::Info,
        key_path(&keys[0]),
        format!(
            "the unquoted value of {} is not valid plain YAML; it was quoted",
            keys.iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}
