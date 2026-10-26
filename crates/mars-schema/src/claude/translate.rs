//! Claude frontmatter into schema v1: the skills and Agents tables of the import contract.
//! Bodies and payload are never touched.

use serde_yaml::{Mapping, Value};

use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
use crate::frontmatter::Frontmatter;
use crate::invocability::find_invocability_field;
use crate::item::DefinitionKind;
use crate::lift::{claude_task_to_agent, lift_foreign_tools};
use crate::mcp::{ForeignMcpDialect, parse_foreign_mcp_token};
use crate::tool_policy;

/// Skill keys Claude reads and schema v1 does not define. They move to
/// `harness-overrides.claude`, which only `.claude/` output receives. (`model` and `effort`
/// have v1 meaning on Agents only; a skill carries neither.) `tools` and the `allowed-tools`
/// family have their own warning.
pub(crate) const CLAUDE_ONLY_SKILL_KEYS: &[&str] = &[
    "argument-hint",
    "arguments",
    "model",
    "effort",
    "context",
    "agent",
    "hooks",
    "paths",
    "shell",
];

pub(crate) const CLAUDE_ONLY_AGENT_KEYS: &[&str] = &[
    "color",
    "initialPrompt",
    "maxTurns",
    "permissionMode",
    "memory",
    "background",
    "isolation",
    "hooks",
];

pub(crate) fn lift(
    kind: DefinitionKind,
    fm: &mut Frontmatter,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> bool {
    match kind {
        DefinitionKind::Skill => lift_skill(fm, diagnostics),
        DefinitionKind::Agent => lift_agent(fm, diagnostics),
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

/// Move the listed keys, in file order and unchanged, under `harness-overrides.claude`. An
/// override already written there wins over a moved key of the same name.
pub(crate) fn move_to_claude_overrides(
    fm: &mut Frontmatter,
    keys: &[&str],
    code: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let moved = fm
        .keys()
        .into_iter()
        .filter(|key| keys.contains(&key.as_str()))
        .filter_map(|key| fm.remove(&key).map(|value| (key, value)))
        .collect::<Vec<_>>();
    if moved.is_empty() {
        return false;
    }
    let mut overrides = match fm.remove("harness-overrides") {
        Some(Value::Mapping(existing)) => existing,
        _ => Mapping::new(),
    };
    let claude_key = Value::String("claude".into());
    let mut claude = match overrides.remove(&claude_key) {
        Some(Value::Mapping(existing)) => existing,
        _ => Mapping::new(),
    };
    let names = moved
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    for (key, value) in moved.iter().cloned() {
        claude.entry(Value::String(key)).or_insert(value);
    }
    overrides.insert(claude_key, Value::Mapping(claude));
    fm.insert("harness-overrides", Value::Mapping(overrides));
    diagnostics.push(SchemaDiagnostic::new(
        code,
        Severity::Info,
        Vec::new(),
        format!("moved Claude-only keys to harness-overrides.claude: {names}"),
    ));
    true
}

fn remove_keys(
    fm: &mut Frontmatter,
    keys: impl IntoIterator<Item = &'static str>,
) -> Vec<&'static str> {
    keys.into_iter()
        .filter(|key| fm.remove(key).is_some())
        .collect()
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn lift_skill(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) -> bool {
    let mut changed = false;
    let rendered = fm.render();
    if rendered.contains("${CLAUDE_PLUGIN_ROOT}") || rendered.contains("${CLAUDE_SKILL_DIR}") {
        warn(
            diagnostics,
            "skill.plugin-variable",
            "refers to `${CLAUDE_PLUGIN_ROOT}` or `${CLAUDE_SKILL_DIR}`, which are left as written and do not resolve outside Claude Code".into(),
        );
    }

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
    changed |= !remove_keys(fm, ["invocation", "allow_implicit_invocation"]).is_empty();

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

    // `allowed-tools` is a turn-scoped grant that restricts nothing, and schema v1 has no
    // skill tool rules, so the keys go rather than becoming a restriction.
    let names_bash = ["allowed-tools", "allowed_tools"]
        .iter()
        .filter_map(|key| fm.get(key))
        .any(|value| {
            tool_policy::tool_entries(value)
                .iter()
                .any(|entry| entry.starts_with("Bash"))
        });
    let dropped = remove_keys(fm, crate::skill::tool_keys());
    if !dropped.is_empty() {
        changed = true;
        warn(
            diagnostics,
            "skill.tools-dropped",
            format!(
                "removed {}: schema v1 has no skill tool rules, and Claude's `allowed-tools` grants without restricting",
                dropped.join(", ")
            ),
        );
    }
    if names_bash || fm.body().contains("scripts/") {
        warn(
            diagnostics,
            "skill.needs-scripts",
            "runs scripts; the instructions load and the scripts stay readable, but the parts that run code will not work until scripts can run".into(),
        );
    }

    changed |= move_to_claude_overrides(
        fm,
        CLAUDE_ONLY_SKILL_KEYS,
        "skill.claude-keys-moved",
        diagnostics,
    );
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

fn lift_agent(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) -> bool {
    let mut changed = false;
    let mut denied = Vec::new();
    for key in ["disallowedTools", "disallowed-tools", "disallowed_tools"] {
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
                diagnostics,
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
                .or_else(|| claude_task_to_agent(&entry))
                .unwrap_or(entry);
            tools.insert(Value::String(name), Value::String("deny".into()));
        }
        fm.insert("tools", Value::Mapping(tools));
    }

    if fm
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.trim().eq_ignore_ascii_case("inherit"))
    {
        fm.remove("model");
        changed = true;
    }

    changed |= move_to_claude_overrides(
        fm,
        CLAUDE_ONLY_AGENT_KEYS,
        "agent.claude-keys-moved",
        diagnostics,
    );
    changed
}

/// Quote an unquoted `description:` whose value holds a colon: the most common invalid YAML
/// in real skills. Returns the repaired document, or `None` when there is nothing to quote.
pub(crate) fn repair_description(text: &str) -> Option<String> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return None;
    }
    let close = lines
        .iter()
        .skip(1)
        .position(|line| line.trim_end() == "---")?
        + 1;
    let index = (1..close).find(|&index| lines[index].starts_with("description:"))?;
    let value = lines[index]["description:".len()..].trim();
    let plain = !value.is_empty()
        && !value.starts_with(['"', '\'', '|', '>', '[', '{', '&', '*', '!', '#']);
    if !plain || !(value.contains(": ") || value.ends_with(':')) {
        return None;
    }
    // Continuation lines of a plain scalar are indented and fold into one line.
    let mut folded = value.to_owned();
    let mut next = index + 1;
    while next < close && lines[next].starts_with([' ', '\t']) && !lines[next].trim().is_empty() {
        folded.push(' ');
        folded.push_str(lines[next].trim());
        next += 1;
    }
    let quoted = serde_json::to_string(&folded).ok()?;
    let mut repaired = String::with_capacity(text.len() + 8);
    lines[..index]
        .iter()
        .for_each(|line| repaired.push_str(line));
    repaired.push_str(&format!("description: {quoted}\n"));
    lines[next..]
        .iter()
        .for_each(|line| repaired.push_str(line));
    Some(repaired)
}

pub(crate) fn repaired_diagnostic() -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "skill.description-repaired",
        Severity::Warning,
        key_path("description"),
        "the unquoted `description` held a colon, which YAML cannot read; it was quoted",
    )
}
