//! Mars tool-name grammar and target-native names.
//!
//! A tool entry is a *capability* (`bash`, `web`, …) with an optional *pattern*
//! (`bash(git status *)`). [`parse_mars_tool_name`] canonicalizes the capability and
//! normalizes the pattern; [`native_names`] answers what a harness calls it.

use crate::compiler::mcp_ref::{MCP_TOOL_NAME_GRAMMAR, try_parse_mcp_tool_name};

const TOOL_NAME_ALLOWED: &str =
    "non-empty tool name; known tools use snake_case and scoped payloads use tool(pattern)";
const TOOL_PATTERN_ALLOWED: &str =
    "scoped payload written as tool(pattern) with a non-empty pattern";
const WILDCARD_ALLOWED: &str = "`*` stands alone; it takes no pattern";

/// Capability of the rule that applies to every tool not otherwise named (allowlists).
pub(crate) const ANY_CAPABILITY: &str = "*";
/// Capability of MCP rules; the pattern is the canonical `server/tool` reference.
pub(crate) const MCP_CAPABILITY: &str = "mcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedToolName {
    /// Canonical snake_case capability, `*`, `mcp`, or the author's spelling when unknown.
    pub capability: String,
    /// Scope inside `capability(...)`, already normalized (`X:*` → `X *`).
    pub pattern: Option<String>,
    pub known: bool,
}

impl ParsedToolName {
    /// The canonical `capability` / `capability(pattern)` spelling.
    #[cfg(test)]
    pub fn render(&self) -> String {
        render_scoped(&self.capability, self.pattern.as_deref())
    }
}

pub(crate) fn render_scoped(name: &str, pattern: Option<&str>) -> String {
    match pattern {
        Some(pattern) => format!("{name}({pattern})"),
        None => name.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolNameParseError {
    Empty,
    InvalidMcpRef,
    UnbalancedPattern,
    EmptyPattern,
    WildcardPattern,
}

impl ToolNameParseError {
    pub fn allowed(&self) -> &'static str {
        match self {
            Self::Empty => TOOL_NAME_ALLOWED,
            Self::InvalidMcpRef => MCP_TOOL_NAME_GRAMMAR,
            Self::UnbalancedPattern | Self::EmptyPattern => TOOL_PATTERN_ALLOWED,
            Self::WildcardPattern => WILDCARD_ALLOWED,
        }
    }
}

/// Whether Mars can name a capability on a harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeStatus {
    /// Mars knows the native name(s).
    Known,
    /// The harness has no such tool, so there is nothing to allow or deny.
    Absent,
    /// Mars cannot name it for this harness. A reported gap, never a guess.
    Unknown,
}

/// Native tool names for one capability on one harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilityNatives {
    pub names: Vec<String>,
    pub status: NativeStatus,
}

struct CanonicalTool {
    name: &'static str,
    aliases: &'static [&'static str],
}

const CANONICAL_TOOLS: &[CanonicalTool] = &[
    CanonicalTool {
        name: "bash",
        aliases: &["shell", "terminal", "exec_command", "shell_command"],
    },
    CanonicalTool {
        name: "read",
        aliases: &["cat", "view", "file_read"],
    },
    CanonicalTool {
        name: "write",
        aliases: &["file_write", "apply_patch"],
    },
    CanonicalTool {
        name: "edit",
        aliases: &["sed", "str_replace"],
    },
    // Sub-agents (Claude `Agent`, OpenCode `task`). Not the task-list tools: see `task`.
    CanonicalTool {
        name: "agent",
        aliases: &["subagent", "spawn_agent"],
    },
    // Task-list tools (Claude `TaskCreate`…`TaskUpdate`, OpenCode `todowrite`/`todoread`,
    // Codex `update_plan`). One capability: a profile gates the list, not each verb.
    CanonicalTool {
        name: "task",
        aliases: &[
            "todo_read",
            "todoread",
            "todo_write",
            "todowrite",
            "task_create",
            "taskcreate",
            "task_get",
            "taskget",
            "task_list",
            "tasklist",
            "task_update",
            "taskupdate",
            "update_plan",
        ],
    },
    // Scoped payloads like skill(init) gate a specific skill via the harness skill tool.
    // Recognition here only affects name normalization/projection — not whether a harness
    // enforces disallowed_tools at runtime (Meridian-side).
    CanonicalTool {
        name: "skill",
        aliases: &[],
    },
    CanonicalTool {
        name: "workflow",
        aliases: &[],
    },
    CanonicalTool {
        name: "glob",
        aliases: &["find"],
    },
    CanonicalTool {
        name: "grep",
        aliases: &["rg", "search", "ripgrep"],
    },
    CanonicalTool {
        name: "notebook",
        aliases: &["jupyter", "notebook_edit", "notebookedit"],
    },
    // `web` = `web_search` + `web_fetch`: the capability profiles write for "reach the web".
    CanonicalTool {
        name: "web",
        aliases: &[],
    },
    CanonicalTool {
        name: "web_search",
        aliases: &["websearch"],
    },
    CanonicalTool {
        name: "web_fetch",
        aliases: &["webfetch", "fetch", "curl"],
    },
    CanonicalTool {
        name: "ask_user",
        aliases: &[
            "askuser",
            "request_user_input",
            "ask_question",
            "ask_user_question",
            "askuserquestion",
        ],
    },
    CanonicalTool {
        name: "cron",
        aliases: &[],
    },
    CanonicalTool {
        name: "notifications",
        aliases: &["pushnotification", "push_notification"],
    },
    CanonicalTool {
        name: "plan_mode",
        aliases: &["planmode", "switch_mode"],
    },
    CanonicalTool {
        name: "worktree",
        aliases: &[],
    },
    CanonicalTool {
        name: "lsp",
        aliases: &[],
    },
    CanonicalTool {
        name: "monitor",
        aliases: &[],
    },
    CanonicalTool {
        name: "send_user_file",
        aliases: &["senduserfile"],
    },
    CanonicalTool {
        name: "schedule_wakeup",
        aliases: &["schedulewakeup"],
    },
    CanonicalTool {
        name: "remote_trigger",
        aliases: &["remotetrigger"],
    },
    CanonicalTool {
        name: "tool_search",
        aliases: &["toolsearch"],
    },
];

struct SemanticOverride {
    canonical: &'static str,
    harness: crate::compiler::agents::HarnessKind,
    native: &'static str,
}

const SEMANTIC_OVERRIDES: &[SemanticOverride] = &[
    // Codex — tool names verified from codex-rs source (openai/codex), 2026-06-22
    // exec_command = shell execution, apply_patch = file edits/writes,
    // spawn_agent = sub-agents. No separate file_read/file_write tools exist.
    SemanticOverride {
        canonical: "bash",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "exec_command",
    },
    SemanticOverride {
        canonical: "read",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "exec_command",
    },
    SemanticOverride {
        canonical: "write",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "apply_patch",
    },
    SemanticOverride {
        canonical: "edit",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "apply_patch",
    },
    SemanticOverride {
        canonical: "agent",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "spawn_agent",
    },
    SemanticOverride {
        canonical: "ask_user",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "request_user_input",
    },
    SemanticOverride {
        canonical: "todo_write",
        harness: crate::compiler::agents::HarnessKind::Codex,
        native: "update_plan",
    },
    SemanticOverride {
        canonical: "read",
        harness: crate::compiler::agents::HarnessKind::OpenCode,
        native: "view",
    },
    SemanticOverride {
        canonical: "web_search",
        harness: crate::compiler::agents::HarnessKind::OpenCode,
        native: "browser",
    },
    SemanticOverride {
        canonical: "web_fetch",
        harness: crate::compiler::agents::HarnessKind::OpenCode,
        native: "fetch",
    },
    // Cursor — PascalCase convention covers most, but these names diverge
    SemanticOverride {
        canonical: "bash",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "Shell",
    },
    SemanticOverride {
        canonical: "edit",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "StrReplace",
    },
    SemanticOverride {
        canonical: "agent",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "Task",
    },
    SemanticOverride {
        canonical: "ask_user",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "AskQuestion",
    },
    SemanticOverride {
        canonical: "plan_mode",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "SwitchMode",
    },
    SemanticOverride {
        canonical: "notebook",
        harness: crate::compiler::agents::HarnessKind::Cursor,
        native: "EditNotebook",
    },
    // Pi — lowercase convention covers most, but glob is called find
    SemanticOverride {
        canonical: "glob",
        harness: crate::compiler::agents::HarnessKind::Pi,
        native: "find",
    },
];

pub(crate) fn parse_mars_tool_name(raw: &str) -> Result<ParsedToolName, ToolNameParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolNameParseError::Empty);
    }

    let (head, payload) = split_tool_name(trimmed);
    let head = head.trim();
    if head.is_empty() {
        return Err(ToolNameParseError::Empty);
    }

    if let Some(mcp_ref) = try_parse_mcp_tool_name(trimmed) {
        return Ok(ParsedToolName {
            capability: MCP_CAPABILITY.to_string(),
            pattern: Some(mcp_ref.to_pattern()),
            known: true,
        });
    }

    if head.eq_ignore_ascii_case("mcp") && payload.trim().starts_with('(') {
        return Err(ToolNameParseError::InvalidMcpRef);
    }

    let pattern = parse_pattern(payload)?;

    if head == ANY_CAPABILITY {
        if pattern.is_some() {
            return Err(ToolNameParseError::WildcardPattern);
        }
        return Ok(ParsedToolName {
            capability: ANY_CAPABILITY.to_string(),
            pattern: None,
            known: true,
        });
    }

    let canonical = canonicalize_head(head);
    Ok(ParsedToolName {
        capability: canonical.name,
        pattern,
        known: canonical.known,
    })
}

fn parse_pattern(payload: &str) -> Result<Option<String>, ToolNameParseError> {
    let payload = payload.trim();
    if payload.is_empty() {
        return Ok(None);
    }
    let inner = payload
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or(ToolNameParseError::UnbalancedPattern)?
        .trim();
    if inner.is_empty() {
        return Err(ToolNameParseError::EmptyPattern);
    }
    Ok(Some(normalize_pattern(inner)))
}

/// Claude's legacy prefix form `git revert:*` and the current `git revert *` mean the same
/// thing, but OpenCode reads `:` literally. Mars writes only the space form.
fn normalize_pattern(pattern: &str) -> String {
    match pattern.strip_suffix(":*") {
        Some(prefix) => format!("{prefix} *"),
        None => pattern.to_string(),
    }
}

/// Native names for `capability` on `harness`.
pub(crate) fn native_names(
    capability: &str,
    harness: crate::compiler::agents::HarnessKind,
) -> CapabilityNatives {
    if is_mcp_shaped(capability) {
        // Reserved MCP wire token: passed through verbatim, casing included.
        return CapabilityNatives {
            names: vec![capability.to_string()],
            status: NativeStatus::Known,
        };
    }

    let known = canonical_tool_name(capability).is_some();
    // Interim: `web` and `task` project like the single names they replaced
    // (`web_search`, `todo_write`) until per-harness one-to-many tables land.
    let capability = match capability {
        "web" => "web_search",
        "task" => "todo_write",
        other => other,
    };
    let descriptor = crate::compiler::harness_descriptor::descriptor(harness);

    if !known {
        let name = match descriptor.tool_naming {
            crate::compiler::harness_descriptor::ToolNamingConvention::PascalCase => {
                snake_to_pascal(capability)
            }
            crate::compiler::harness_descriptor::ToolNamingConvention::SnakeCase
            | crate::compiler::harness_descriptor::ToolNamingConvention::Lowercase => {
                capability.to_string()
            }
        };
        return CapabilityNatives {
            names: vec![name],
            status: NativeStatus::Unknown,
        };
    }

    let name = semantic_override(capability, descriptor.kind)
        .map(str::to_string)
        .unwrap_or_else(|| match descriptor.tool_naming {
            crate::compiler::harness_descriptor::ToolNamingConvention::PascalCase => {
                snake_to_pascal(capability)
            }
            crate::compiler::harness_descriptor::ToolNamingConvention::SnakeCase => {
                capability.to_string()
            }
            crate::compiler::harness_descriptor::ToolNamingConvention::Lowercase => {
                strip_underscores(capability)
            }
        });
    CapabilityNatives {
        names: vec![name],
        status: NativeStatus::Known,
    }
}

struct CanonicalizedHead {
    name: String,
    known: bool,
}

fn split_tool_name(value: &str) -> (&str, &str) {
    match value.find('(') {
        Some(index) => (&value[..index], &value[index..]),
        None => (value, ""),
    }
}

/// Returns true when `head` starts with the reserved MCP wire prefix `mcp__` (case-insensitive).
///
/// Any `mcp__`-prefixed unknown name is passed verbatim to harnesses — the prefix is reserved
/// on the MCP wire, so preserving exact casing is intentional. We deliberately do not
/// validate server/tool segment shape here (including whole-server `mcp__server__*` and global
/// `mcp__*` forms); per-harness MCP projection is a separate planned feature.
fn is_mcp_shaped(head: &str) -> bool {
    head.trim()
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("mcp__"))
}

fn canonicalize_head(head: &str) -> CanonicalizedHead {
    if is_mcp_shaped(head) {
        return CanonicalizedHead {
            name: head.to_string(),
            known: false,
        };
    }

    let lowercase = head.to_ascii_lowercase();

    if let Some(canonical) = canonical_tool_name(&lowercase) {
        return known(canonical);
    }

    if let Some(canonical) = canonical_alias(&lowercase) {
        return known(canonical);
    }

    if head.contains('_') {
        return CanonicalizedHead {
            name: lowercase,
            known: false,
        };
    }

    if is_all_caps(head) {
        return CanonicalizedHead {
            name: lowercase,
            known: false,
        };
    }

    if is_mixed_case(head) {
        let snake = pascal_to_snake(head);
        if let Some(canonical) = canonical_tool_name(&snake) {
            return known(canonical);
        }
        return CanonicalizedHead {
            name: snake,
            known: false,
        };
    }

    CanonicalizedHead {
        name: head.to_string(),
        known: false,
    }
}

fn known(name: &'static str) -> CanonicalizedHead {
    CanonicalizedHead {
        name: name.to_string(),
        known: true,
    }
}

fn canonical_tool_name(name: &str) -> Option<&'static str> {
    CANONICAL_TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.name)
}

fn canonical_alias(alias: &str) -> Option<&'static str> {
    CANONICAL_TOOLS
        .iter()
        .find(|tool| tool.aliases.contains(&alias))
        .map(|tool| tool.name)
}

fn semantic_override(
    canonical: &str,
    harness: crate::compiler::agents::HarnessKind,
) -> Option<&'static str> {
    SEMANTIC_OVERRIDES
        .iter()
        .find(|override_entry| {
            override_entry.canonical == canonical && override_entry.harness == harness
        })
        .map(|override_entry| override_entry.native)
}

fn is_all_caps(s: &str) -> bool {
    let mut has_upper = false;
    for ch in s.chars().filter(|ch| ch.is_ascii_alphabetic()) {
        if ch.is_ascii_lowercase() {
            return false;
        }
        has_upper = true;
    }
    has_upper
}

fn is_mixed_case(s: &str) -> bool {
    s.chars().any(|ch| ch.is_ascii_uppercase()) && s.chars().any(|ch| ch.is_ascii_lowercase())
}

fn pascal_to_snake(s: &str) -> String {
    let mut out = String::new();
    for (index, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn snake_to_pascal(s: &str) -> String {
    if s == "lsp" {
        return "LSP".to_string();
    }

    let mut out = String::new();
    for part in s.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
}

fn strip_underscores(s: &str) -> String {
    s.replace('_', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> ParsedToolName {
        parse_mars_tool_name(raw).unwrap()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ToolProjectionStatus {
        Known,
        UnknownProjected,
        McpVerbatim,
    }

    struct ProjectedToolName {
        name: String,
        status: ToolProjectionStatus,
    }

    fn project(raw: &str, harness: &str) -> ProjectedToolName {
        let harness = crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
            .expect("test harness exists")
            .kind;
        let parsed = parse(raw);
        let natives = native_names(&parsed.capability, harness);
        let status = match natives.status {
            _ if is_mcp_shaped(&parsed.capability) => ToolProjectionStatus::McpVerbatim,
            NativeStatus::Known => ToolProjectionStatus::Known,
            NativeStatus::Absent | NativeStatus::Unknown => ToolProjectionStatus::UnknownProjected,
        };
        ProjectedToolName {
            name: render_scoped(&natives.names[0], parsed.pattern.as_deref()),
            status,
        }
    }

    #[test]
    fn parses_known_tools_and_aliases_to_snake_case() {
        let cases = [
            ("bash", "bash", true),
            ("Bash", "bash", true),
            ("BASH", "bash", true),
            ("shell", "bash", true),
            ("ask_user", "ask_user", true),
            ("AskUser", "ask_user", true),
            ("askuser", "ask_user", true),
            ("LSP", "lsp", true),
            ("WebSearch", "web_search", true),
            ("cat", "read", true),
            ("rg", "grep", true),
            ("CustomTool", "custom_tool", false),
            ("my_custom", "my_custom", false),
            ("bash(git *)", "bash(git *)", true),
            ("skill(init)", "skill(init)", true),
            ("Skill(deep-research)", "skill(deep-research)", true),
            ("workflow", "workflow", true),
            ("web", "web", true),
            ("websearch", "web_search", true),
            ("webfetch", "web_fetch", true),
            ("AskUserQuestion", "ask_user", true),
            ("NotebookEdit", "notebook", true),
            ("TaskCreate", "task", true),
            ("TodoWrite", "task", true),
            ("update_plan", "task", true),
            ("subagent", "agent", true),
        ];

        for (raw, expected_name, expected_known) in cases {
            let parsed = parse(raw);
            assert_eq!(parsed.render(), expected_name, "raw: {raw}");
            assert_eq!(parsed.known, expected_known, "raw: {raw}");
        }
    }

    #[test]
    fn task_means_task_list_not_sub_agents() {
        // `task` was an alias of `agent`. Shipped profiles write both `agent: deny` and
        // `task: deny`, and they name different tools.
        assert_eq!(parse("task").capability, "task");
        assert_eq!(parse("Task").capability, "task");
        assert_eq!(parse("agent").capability, "agent");
        assert_ne!(parse("task").capability, parse("agent").capability);
    }

    #[test]
    fn rejects_empty_tool_names() {
        assert_eq!(parse_mars_tool_name(""), Err(ToolNameParseError::Empty));
        assert_eq!(
            parse_mars_tool_name("(git *)"),
            Err(ToolNameParseError::Empty)
        );
    }

    #[test]
    fn target_projection_maps_canonical_to_native_by_convention_and_override() {
        let cases = [
            ("bash", "claude", "Bash", ToolProjectionStatus::Known),
            ("bash", "codex", "exec_command", ToolProjectionStatus::Known),
            ("bash", "opencode", "bash", ToolProjectionStatus::Known),
            ("read", "opencode", "view", ToolProjectionStatus::Known),
            ("read", "codex", "exec_command", ToolProjectionStatus::Known),
            (
                "web_search",
                "claude",
                "WebSearch",
                ToolProjectionStatus::Known,
            ),
            (
                "web_search",
                "codex",
                "web_search",
                ToolProjectionStatus::Known,
            ),
            (
                "web_search",
                "opencode",
                "browser",
                ToolProjectionStatus::Known,
            ),
            ("ask_user", "claude", "AskUser", ToolProjectionStatus::Known),
            (
                "ask_user",
                "codex",
                "request_user_input",
                ToolProjectionStatus::Known,
            ),
            ("lsp", "claude", "LSP", ToolProjectionStatus::Known),
            (
                "CustomTool",
                "claude",
                "CustomTool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "bash(git *)",
                "claude",
                "Bash(git *)",
                ToolProjectionStatus::Known,
            ),
            (
                "bash(git *)",
                "codex",
                "exec_command(git *)",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(init)",
                "claude",
                "Skill(init)",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(deep-research)",
                "codex",
                "skill(deep-research)",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(init)",
                "cursor",
                "Skill(init)",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(init)",
                "opencode",
                "skill(init)",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(init)",
                "pi",
                "skill(init)",
                ToolProjectionStatus::Known,
            ),
            (
                "workflow",
                "claude",
                "Workflow",
                ToolProjectionStatus::Known,
            ),
            ("workflow", "codex", "workflow", ToolProjectionStatus::Known),
            ("web", "claude", "WebSearch", ToolProjectionStatus::Known),
            ("web", "codex", "web_search", ToolProjectionStatus::Known),
            ("web", "opencode", "browser", ToolProjectionStatus::Known),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "raw: {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "raw: {raw}, harness: {harness}"
            );
        }
    }

    #[test]
    fn projection_accepts_input_aliases_and_pascal_case() {
        assert_eq!(project("Bash", "codex").name, "exec_command");
        assert_eq!(project("shell", "claude").name, "Bash");
        assert_eq!(project("WebSearch", "opencode").name, "browser");
        assert_eq!(project("BASH(git *)", "codex").name, "exec_command(git *)");
    }

    #[test]
    fn cursor_projection_uses_semantic_overrides() {
        let cases = [
            ("bash", "cursor", "Shell", ToolProjectionStatus::Known),
            ("edit", "cursor", "StrReplace", ToolProjectionStatus::Known),
            ("agent", "cursor", "Task", ToolProjectionStatus::Known),
            (
                "ask_user",
                "cursor",
                "AskQuestion",
                ToolProjectionStatus::Known,
            ),
            (
                "plan_mode",
                "cursor",
                "SwitchMode",
                ToolProjectionStatus::Known,
            ),
            (
                "notebook",
                "cursor",
                "EditNotebook",
                ToolProjectionStatus::Known,
            ),
            // Convention handles the rest
            ("read", "cursor", "Read", ToolProjectionStatus::Known),
            ("write", "cursor", "Write", ToolProjectionStatus::Known),
            ("grep", "cursor", "Grep", ToolProjectionStatus::Known),
            ("glob", "cursor", "Glob", ToolProjectionStatus::Known),
            (
                "web_search",
                "cursor",
                "WebSearch",
                ToolProjectionStatus::Known,
            ),
            (
                "web_fetch",
                "cursor",
                "WebFetch",
                ToolProjectionStatus::Known,
            ),
            (
                "todo_write",
                "cursor",
                "TodoWrite",
                ToolProjectionStatus::Known,
            ),
            (
                "skill(init)",
                "cursor",
                "Skill(init)",
                ToolProjectionStatus::Known,
            ),
            (
                "workflow",
                "cursor",
                "Workflow",
                ToolProjectionStatus::Known,
            ),
            ("web", "cursor", "WebSearch", ToolProjectionStatus::Known),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "raw: {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "raw: {raw}, harness: {harness}"
            );
        }
    }

    #[test]
    fn pi_projection_uses_lowercase_convention_and_overrides() {
        let cases = [
            ("bash", "pi", "bash", ToolProjectionStatus::Known),
            ("read", "pi", "read", ToolProjectionStatus::Known),
            ("write", "pi", "write", ToolProjectionStatus::Known),
            ("edit", "pi", "edit", ToolProjectionStatus::Known),
            ("grep", "pi", "grep", ToolProjectionStatus::Known),
            ("glob", "pi", "find", ToolProjectionStatus::Known), // semantic override
            ("ask_user", "pi", "askuser", ToolProjectionStatus::Known), // convention strips _
            (
                "skill(init)",
                "pi",
                "skill(init)",
                ToolProjectionStatus::Known,
            ),
            ("workflow", "pi", "workflow", ToolProjectionStatus::Known),
            ("web", "pi", "websearch", ToolProjectionStatus::Known),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "raw: {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "raw: {raw}, harness: {harness}"
            );
        }
    }

    #[test]
    fn unknown_plain_tools_project_via_harness_convention() {
        let cases = [
            (
                "my_custom_tool",
                "claude",
                "MyCustomTool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "my_custom_tool",
                "cursor",
                "MyCustomTool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "my_custom_tool",
                "codex",
                "my_custom_tool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "my_custom_tool",
                "opencode",
                "my_custom_tool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "my_custom_tool",
                "pi",
                "my_custom_tool",
                ToolProjectionStatus::UnknownProjected,
            ),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "raw: {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "raw: {raw}, harness: {harness}"
            );
        }
    }

    #[test]
    fn mcp_shaped_tools_preserve_verbatim_casing() {
        let cases = [
            (
                "mcp__github__CreateIssue",
                "claude",
                "mcp__github__CreateIssue",
                ToolProjectionStatus::McpVerbatim,
            ),
            (
                "mcp__github__CreateIssue",
                "codex",
                "mcp__github__CreateIssue",
                ToolProjectionStatus::McpVerbatim,
            ),
            (
                "MCP__Server__Tool(scope)",
                "claude",
                "MCP__Server__Tool(scope)",
                ToolProjectionStatus::McpVerbatim,
            ),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "raw: {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "raw: {raw}, harness: {harness}"
            );
        }
    }

    #[test]
    fn parse_preserves_mcp_tool_segment_casing() {
        let parsed = parse("mcp__github__CreateIssue");
        assert_eq!(parsed.render(), "mcp__github__CreateIssue");
        assert!(!parsed.known);
    }

    #[test]
    fn parse_recognizes_valid_mcp_scoped_refs() {
        let parsed = parse("mcp(GitHub/CreateIssue)");
        assert_eq!(parsed.capability, "mcp");
        assert_eq!(parsed.pattern.as_deref(), Some("GitHub/CreateIssue"));
        assert!(parsed.known);

        let parsed = parse("mcp(github)");
        assert_eq!(parsed.pattern.as_deref(), Some("github/*"));
        assert!(parsed.known);
    }

    #[test]
    fn rejects_malformed_mcp_scoped_refs() {
        for raw in ["mcp()", "mcp(/x)", "mcp(x/)", "mcp(a/b/c)"] {
            assert_eq!(
                parse_mars_tool_name(raw),
                Err(ToolNameParseError::InvalidMcpRef),
                "expected rejection for {raw}"
            );
        }
    }

    #[test]
    fn malformed_mcp_in_disallowed_tools_is_validation_error() {
        let mut diags = Vec::new();
        let yaml = "---\nname: a\ndescription: d\ndisallowed-tools: [mcp()]\n---\n";
        let fm = crate::frontmatter::Frontmatter::parse(yaml).unwrap();
        let mut push = |field: &str, value: &str, allowed: &'static str| {
            diags.push((field.to_string(), value.to_string(), allowed));
        };
        let tools = crate::compiler::tool_policy::yaml_tool_list(
            "disallowed-tools",
            fm.get("disallowed-tools").unwrap(),
            &mut push,
        );
        assert!(tools.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].0, "disallowed-tools[0]");
        assert_eq!(diags[0].1, "mcp()");
    }

    #[test]
    fn mcp_wire_form_projects_as_verbatim() {
        let wire = project("mcp__github__create_issue", "claude");
        assert_eq!(wire.name, "mcp__github__create_issue");
        assert_eq!(wire.status, ToolProjectionStatus::McpVerbatim);
    }

    #[test]
    fn non_ascii_unknown_tools_do_not_panic_and_stay_convention_projected() {
        let cases = [
            (
                "café_tool",
                "claude",
                "CaféTool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "🎉🎉tool",
                "claude",
                "🎉🎉tool",
                ToolProjectionStatus::UnknownProjected,
            ),
            (
                "🚀tool",
                "codex",
                "🚀tool",
                ToolProjectionStatus::UnknownProjected,
            ),
        ];

        for (raw, harness, expected_name, expected_status) in cases {
            let parsed = parse_mars_tool_name(raw).expect("parse should not panic");
            assert_eq!(parsed.render(), raw, "parse name for {raw}");
            assert!(!parsed.known, "parse known flag for {raw}");

            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "projected name for {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status, expected_status,
                "projected status for {raw}, harness: {harness}"
            );
            assert_ne!(
                projected.status,
                ToolProjectionStatus::McpVerbatim,
                "{raw} must not be treated as MCP verbatim"
            );
        }
    }
}
