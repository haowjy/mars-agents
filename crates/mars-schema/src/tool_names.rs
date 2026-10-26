//! Mars tool-name grammar.
//!
//! A tool entry is a *capability* (`bash`, `web`, …) with an optional *pattern*
//! (`bash(git status *)`). [`parse_mars_tool_name`] canonicalizes the capability and
//! normalizes the pattern. What a harness calls a capability lives in
//! [`crate::tool_natives`].

use serde::{Deserialize, Serialize};

use crate::mcp::{MCP_TOOL_NAME_GRAMMAR, McpRef, try_parse_mcp_tool_name};

const TOOL_NAME_ALLOWED: &str =
    "non-empty tool name; known tools use snake_case and scoped payloads use tool(pattern)";
const TOOL_PATTERN_ALLOWED: &str =
    "scoped payload written as tool(pattern) with a non-empty pattern";
const WILDCARD_ALLOWED: &str = "`*` stands alone; it takes no pattern";

/// What a tool-policy rule is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    /// `*`: every tool not otherwise named. An explicit `'*': deny` makes an allowlist.
    Any,
    /// `mcp(server/tool)`.
    Mcp(McpRef),
    /// A capability in the vocabulary below.
    Builtin(&'static str),
    /// The author's spelling of a name Mars has no row for, or an `mcp__…` wire token that
    /// passes through verbatim. Only a name that parsed gets here.
    Unknown(String),
}

impl Capability {
    /// The spelling in profiles and in the launch bundle's `capability` field.
    pub fn name(&self) -> &str {
        match self {
            Self::Any => "*",
            Self::Mcp(_) => "mcp",
            Self::Builtin(name) => name,
            Self::Unknown(name) => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedToolName {
    pub capability: Capability,
    /// Scope inside `capability(...)`; a `bash` command prefix is normalized (`X:*` → `X *`).
    /// `None` for `*` and `mcp`, which carry their scope in the capability.
    pub pattern: Option<String>,
}

/// A tool name written in `mars.toml` (`[agents.<name>.tools]`, `[skills.<name>.tools]`),
/// parsed when the config loads: an overlay cannot carry a name a profile would reject.
/// Keeps the author's spelling, so saving `mars.toml` does not rewrite it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OverlayToolName {
    raw: String,
    parsed: ParsedToolName,
}

impl OverlayToolName {
    pub fn parsed(&self) -> &ParsedToolName {
        &self.parsed
    }
}

impl TryFrom<String> for OverlayToolName {
    type Error = String;

    fn try_from(raw: String) -> Result<Self, String> {
        match parse_mars_tool_name(&raw) {
            Ok(parsed) => Ok(Self { raw, parsed }),
            Err(err) => Err(format!(
                "invalid tool name `{raw}`; allowed: {}",
                err.allowed()
            )),
        }
    }
}

impl From<OverlayToolName> for String {
    fn from(name: OverlayToolName) -> Self {
        name.raw
    }
}

pub fn render_scoped(name: &str, pattern: Option<&str>) -> String {
    match pattern {
        Some(pattern) => format!("{name}({pattern})"),
        None => name.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolNameParseError {
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
    // Task-list tools (Claude `TaskCreate`…`TaskUpdate`, OpenCode `todowrite`, Codex
    // `update_plan`). One capability: a profile gates the list, not each verb. `todoread` stays
    // as an input spelling only; OpenCode has no such tool.
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
    // Viewing an image file. Only Codex has a tool for it (`view_image`); elsewhere the read
    // tool opens images.
    CanonicalTool {
        name: "image",
        aliases: &["view_image"],
    },
];

pub fn parse_mars_tool_name(raw: &str) -> Result<ParsedToolName, ToolNameParseError> {
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
            capability: Capability::Mcp(mcp_ref),
            pattern: None,
        });
    }

    // `mcp` is only ever a reference: a bare `mcp`, `MCP` or a malformed `mcp(...)` is an error,
    // not an unknown tool that happens to share the bundle's `mcp` capability name.
    if head.eq_ignore_ascii_case("mcp") {
        return Err(ToolNameParseError::InvalidMcpRef);
    }

    let pattern = parse_pattern(payload)?;

    if head == "*" {
        if pattern.is_some() {
            return Err(ToolNameParseError::WildcardPattern);
        }
        return Ok(ParsedToolName {
            capability: Capability::Any,
            pattern: None,
        });
    }

    let capability = canonicalize_head(head);
    let pattern = pattern.map(|pattern| match capability {
        Capability::Builtin("bash") => normalize_bash_pattern(&pattern),
        _ => pattern,
    });
    Ok(ParsedToolName {
        capability,
        pattern,
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
    Ok(Some(inner.to_string()))
}

/// Claude's legacy command-prefix form `git revert:*` and the current `git revert *` mean
/// the same thing, but OpenCode reads `:` literally. Mars writes only the space form. Only a
/// shell command has a prefix form: `read(./secrets:*)` and `web_fetch(domain:*)` keep the
/// colon, which is part of the path or the host.
fn normalize_bash_pattern(pattern: &str) -> String {
    match pattern.strip_suffix(":*") {
        Some(prefix) => format!("{prefix} *"),
        None => pattern.to_string(),
    }
}

#[cfg(test)]
pub fn is_canonical_capability(name: &str) -> bool {
    canonical_tool_name(name).is_some()
}

pub fn canonical_capabilities() -> impl Iterator<Item = &'static str> {
    CANONICAL_TOOLS.iter().map(|tool| tool.name)
}

/// The authored capability vocabulary, including accepted legacy spellings.
pub fn vocabulary() -> serde_json::Value {
    serde_json::json!({"capabilities": CANONICAL_TOOLS.iter().map(|tool| serde_json::json!({
        "name": tool.name, "aliases": tool.aliases
    })).collect::<Vec<_>>()})
}

/// Split `head(payload)` into `head` and `(payload)`; a name without parentheses has no payload.
pub fn split_tool_name(value: &str) -> (&str, &str) {
    match value.find('(') {
        Some(index) => (&value[..index], &value[index..]),
        None => (value, ""),
    }
}

/// Whether `name` is a Claude-style MCP wire token (`mcp__server__tool`, case-insensitive prefix).
///
/// Such a name passes to harnesses verbatim, casing included: the prefix is reserved on the MCP
/// wire. The server/tool segments are not validated here, so whole-server `mcp__server__*` and
/// global `mcp__*` forms pass through as written.
pub fn is_mcp_wire_token(name: &str) -> bool {
    name.trim()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("mcp__"))
}

fn canonicalize_head(head: &str) -> Capability {
    if is_mcp_wire_token(head) {
        return Capability::Unknown(head.to_string());
    }

    let lowercase = head.to_ascii_lowercase();
    if let Some(canonical) = canonical_tool_name(&lowercase).or_else(|| canonical_alias(&lowercase))
    {
        return Capability::Builtin(canonical);
    }

    if head.contains('_') || is_all_caps(head) {
        return Capability::Unknown(lowercase);
    }

    if is_mixed_case(head) {
        let snake = pascal_to_snake(head);
        return match canonical_tool_name(&snake) {
            Some(canonical) => Capability::Builtin(canonical),
            None => Capability::Unknown(snake),
        };
    }

    Capability::Unknown(head.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> ParsedToolName {
        parse_mars_tool_name(raw).unwrap()
    }

    fn render(parsed: &ParsedToolName) -> String {
        render_scoped(parsed.capability.name(), parsed.pattern.as_deref())
    }

    #[test]
    fn parses_known_tools_and_aliases_to_snake_case() {
        let cases = [
            ("bash", "bash"),
            ("Bash", "bash"),
            ("BASH", "bash"),
            ("shell", "bash"),
            ("ask_user", "ask_user"),
            ("AskUser", "ask_user"),
            ("askuser", "ask_user"),
            ("LSP", "lsp"),
            ("WebSearch", "web_search"),
            ("cat", "read"),
            ("rg", "grep"),
            ("CustomTool", "custom_tool"),
            ("my_custom", "my_custom"),
            ("bash(git *)", "bash(git *)"),
            ("skill(init)", "skill(init)"),
            ("Skill(deep-research)", "skill(deep-research)"),
            ("workflow", "workflow"),
            ("web", "web"),
            ("websearch", "web_search"),
            ("webfetch", "web_fetch"),
            ("AskUserQuestion", "ask_user"),
            ("NotebookEdit", "notebook"),
            ("TaskCreate", "task"),
            ("TodoWrite", "task"),
            ("update_plan", "task"),
            ("subagent", "agent"),
        ];

        for (raw, expected) in cases {
            assert_eq!(render(&parse(raw)), expected, "raw: {raw}");
        }
    }

    #[test]
    fn task_means_task_list_not_sub_agents() {
        // `task` was an alias of `agent`. Shipped profiles write both `agent: deny` and
        // `task: deny`, and they name different tools.
        assert_eq!(parse("task").capability, Capability::Builtin("task"));
        assert_eq!(parse("Task").capability, Capability::Builtin("task"));
        assert_eq!(parse("agent").capability, Capability::Builtin("agent"));
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
    fn star_is_the_any_capability_and_takes_no_pattern() {
        assert_eq!(parse("*").capability, Capability::Any);
        assert_eq!(
            parse_mars_tool_name("*(x)"),
            Err(ToolNameParseError::WildcardPattern)
        );
    }

    #[test]
    fn non_ascii_unknown_tools_do_not_panic() {
        for raw in ["café_tool", "🎉🎉tool", "🚀tool"] {
            let parsed = parse_mars_tool_name(raw).expect("parse should not panic");
            assert_eq!(render(&parsed), raw);
            assert!(matches!(parsed.capability, Capability::Unknown(_)));
        }
    }

    #[test]
    fn mcp_wire_tokens_stay_verbatim_unknown_capabilities() {
        for raw in ["mcp__github__CreateIssue", "MCP__Server__Tool"] {
            let parsed = parse(raw);
            assert_eq!(parsed.capability, Capability::Unknown(raw.to_string()));
            assert!(is_mcp_wire_token(raw));
        }
    }

    #[test]
    fn parse_recognizes_valid_mcp_scoped_refs() {
        let Capability::Mcp(mcp_ref) = parse("mcp(GitHub/CreateIssue)").capability else {
            panic!("expected an MCP capability");
        };
        assert_eq!(mcp_ref.to_pattern(), "GitHub/CreateIssue");

        let Capability::Mcp(mcp_ref) = parse("mcp(github)").capability else {
            panic!("expected an MCP capability");
        };
        assert_eq!(mcp_ref.to_pattern(), "github/*");
        assert_eq!(parse("mcp(github)").pattern, None);
    }

    #[test]
    fn rejects_malformed_and_bare_mcp() {
        // A bare `mcp` or `MCP` names no server, so it is as invalid as `mcp()`.
        for raw in ["mcp()", "mcp(/x)", "mcp(x/)", "mcp(a/b/c)", "mcp", "MCP"] {
            assert_eq!(
                parse_mars_tool_name(raw),
                Err(ToolNameParseError::InvalidMcpRef),
                "expected rejection for {raw}"
            );
        }
    }
}
