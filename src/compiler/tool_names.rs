//! Mars tool-name grammar and target-native names.
//!
//! A tool entry is a *capability* (`bash`, `web`, …) with an optional *pattern*
//! (`bash(git status *)`). [`parse_mars_tool_name`] canonicalizes the capability and
//! normalizes the pattern; [`native_names`] answers what a harness calls it.

use crate::compiler::mcp_ref::{MCP_TOOL_NAME_GRAMMAR, try_parse_mcp_tool_name};
use crate::compiler::tool_natives::Native;

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

impl NativeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Absent => "absent",
            Self::Unknown => "unknown",
        }
    }
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

/// Native names for `capability` on `harness`, from the explicit tables in
/// [`crate::compiler::tool_natives`]. Never derived: a capability without a row is
/// [`NativeStatus::Unknown`] with no names. `scoped` says the rule carries a pattern, which a
/// [`Native::ToolsScopedToFirst`] row applies to its first tool only.
pub(crate) fn native_names(
    capability: &str,
    harness: crate::compiler::agents::HarnessKind,
    scoped: bool,
) -> CapabilityNatives {
    if is_mcp_shaped(capability) {
        // Reserved MCP wire token: passed through verbatim, casing included.
        return CapabilityNatives {
            names: vec![capability.to_string()],
            status: NativeStatus::Known,
        };
    }

    match crate::compiler::tool_natives::lookup(capability, harness) {
        Some(Native::Tools(names)) => CapabilityNatives {
            names: names.iter().map(|name| name.to_string()).collect(),
            status: NativeStatus::Known,
        },
        Some(Native::ToolsScopedToFirst(names)) => CapabilityNatives {
            names: names
                .iter()
                .take(if scoped { 1 } else { names.len() })
                .map(|name| name.to_string())
                .collect(),
            status: NativeStatus::Known,
        },
        Some(Native::Absent) => CapabilityNatives {
            names: Vec::new(),
            status: NativeStatus::Absent,
        },
        None => CapabilityNatives {
            names: Vec::new(),
            status: NativeStatus::Unknown,
        },
    }
}

#[cfg(test)]
pub(crate) fn is_canonical_capability(name: &str) -> bool {
    canonical_tool_name(name).is_some()
}

#[cfg(test)]
pub(crate) fn canonical_capabilities() -> impl Iterator<Item = &'static str> {
    CANONICAL_TOOLS.iter().map(|tool| tool.name)
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

    /// Native names and status of a raw tool entry on a harness, via parse then table.
    fn natives(raw: &str, harness: &str) -> (Vec<String>, NativeStatus) {
        let harness = crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
            .expect("test harness exists")
            .kind;
        let capability = parse(raw).capability;
        let natives = native_names(&capability, harness, false);
        (natives.names, natives.status)
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
    fn aliases_and_native_spellings_reach_the_same_native_tools() {
        let known = |names: &[&str]| {
            (
                names.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
                NativeStatus::Known,
            )
        };
        assert_eq!(
            natives("Bash", "codex"),
            known(&["exec_command", "write_stdin"])
        );
        assert_eq!(natives("shell", "claude"), known(&["Bash"]));
        assert_eq!(natives("exec_command", "claude"), known(&["Bash"]));
        assert_eq!(natives("WebSearch", "opencode"), known(&["websearch"]));
        assert_eq!(
            natives("web", "opencode"),
            known(&["websearch", "webfetch"])
        );
        assert_eq!(
            natives("AskUserQuestion", "claude"),
            known(&["AskUserQuestion"])
        );
        assert_eq!(natives("AskUser", "claude"), known(&["AskUserQuestion"]));
        assert_eq!(natives("askuser", "opencode"), known(&["question"]));
        assert_eq!(
            natives("edit", "claude"),
            known(&["Edit", "Write", "NotebookEdit"])
        );
        assert_eq!(natives("todowrite", "claude").0.len(), 6);
    }

    #[test]
    fn a_tool_the_harness_lacks_is_absent_not_guessed() {
        assert_eq!(natives("read", "codex"), (vec![], NativeStatus::Absent));
        assert_eq!(natives("ask_user", "pi"), (vec![], NativeStatus::Absent));
    }

    #[test]
    fn a_command_pattern_scopes_only_the_tool_that_starts_the_command() {
        use crate::compiler::agents::HarnessKind;
        // `write_stdin` carries input to a session `exec_command` opened, not a command.
        assert_eq!(
            native_names("bash", HarnessKind::Codex, true).names,
            ["exec_command"]
        );
        assert_eq!(
            native_names("bash", HarnessKind::Codex, false).names,
            ["exec_command", "write_stdin"]
        );
    }

    #[test]
    fn a_thing_the_harness_can_do_without_a_tool_to_switch_is_unknown_not_absent() {
        // Codex fetches pages through `web_search`; Codex and Pi load skills by reading files.
        for (capability, harness) in [("web_fetch", "codex"), ("skill", "codex"), ("skill", "pi")] {
            assert_eq!(
                natives(capability, harness),
                (vec![], NativeStatus::Unknown),
                "{harness} {capability}"
            );
        }
    }

    #[test]
    fn a_name_without_a_table_row_is_unknown_with_no_native_name() {
        // Known capability, no Cursor row: no convention fallback fills it in.
        assert_eq!(natives("lsp", "cursor"), (vec![], NativeStatus::Unknown));
        // Not a capability at all.
        for harness in ["claude", "codex", "opencode", "pi", "cursor"] {
            assert_eq!(
                natives("my_custom_tool", harness),
                (vec![], NativeStatus::Unknown),
                "{harness}"
            );
        }
    }

    #[test]
    fn non_ascii_unknown_tools_do_not_panic() {
        for raw in ["café_tool", "🎉🎉tool", "🚀tool"] {
            let parsed = parse_mars_tool_name(raw).expect("parse should not panic");
            assert_eq!(parsed.render(), raw);
            assert!(!parsed.known);
            assert_eq!(natives(raw, "claude"), (vec![], NativeStatus::Unknown));
        }
    }

    #[test]
    fn mcp_wire_tokens_are_verbatim_on_every_harness() {
        for (raw, expected) in [
            ("mcp__github__CreateIssue", "mcp__github__CreateIssue"),
            ("MCP__Server__Tool", "MCP__Server__Tool"),
            ("mcp__github__create_issue", "mcp__github__create_issue"),
        ] {
            for harness in ["claude", "codex", "opencode", "pi", "cursor"] {
                assert_eq!(
                    natives(raw, harness),
                    (vec![expected.to_string()], NativeStatus::Known),
                    "{raw} on {harness}"
                );
            }
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
}
