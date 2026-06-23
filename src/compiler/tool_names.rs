//! Mars tool-name grammar and target-native projection.

use crate::compiler::mcp_ref::{MCP_TOOL_NAME_GRAMMAR, try_parse_mcp_tool_name};

const TOOL_NAME_ALLOWED: &str =
    "non-empty tool name; known tools use snake_case and scoped payloads use tool(pattern)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NamingConvention {
    PascalCase,
    SnakeCase,
    Lowercase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedToolName {
    pub name: String,
    pub known: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolNameParseError {
    Empty,
    InvalidMcpRef,
}

impl ToolNameParseError {
    pub fn allowed(&self) -> &'static str {
        match self {
            Self::Empty => TOOL_NAME_ALLOWED,
            Self::InvalidMcpRef => MCP_TOOL_NAME_GRAMMAR,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectedToolName {
    pub name: String,
    pub status: ToolProjectionStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolProjectionStatus {
    Known,
    UnknownProjected,
    /// Reserved MCP wire token (`mcp__…`) passed through verbatim. Canonical `mcp(...)`
    /// refs are extracted upstream in [`tool_policy`] and projected via [`mcp_ref`].
    McpVerbatim,
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
    CanonicalTool {
        name: "agent",
        aliases: &["subagent", "spawn_agent", "task"],
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
        aliases: &["jupyter"],
    },
    CanonicalTool {
        name: "web_search",
        // `web` is package shorthand for web search (Codex native: web_search, not `web`).
        aliases: &["websearch", "web"],
    },
    CanonicalTool {
        name: "web_fetch",
        aliases: &["webfetch", "fetch", "curl"],
    },
    CanonicalTool {
        name: "ask_user",
        aliases: &["askuser", "request_user_input", "ask_question"],
    },
    CanonicalTool {
        name: "todo_read",
        aliases: &["todoread"],
    },
    CanonicalTool {
        name: "todo_write",
        aliases: &["todowrite"],
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
        aliases: &["planmode", "update_plan", "switch_mode"],
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
    harness: &'static str,
    native: &'static str,
}

const SEMANTIC_OVERRIDES: &[SemanticOverride] = &[
    // Codex — tool names verified from codex-rs source (openai/codex), 2026-06-22
    // exec_command = shell execution, apply_patch = file edits/writes,
    // spawn_agent = sub-agents. No separate file_read/file_write tools exist.
    SemanticOverride {
        canonical: "bash",
        harness: "codex",
        native: "exec_command",
    },
    SemanticOverride {
        canonical: "read",
        harness: "codex",
        native: "exec_command",
    },
    SemanticOverride {
        canonical: "write",
        harness: "codex",
        native: "apply_patch",
    },
    SemanticOverride {
        canonical: "edit",
        harness: "codex",
        native: "apply_patch",
    },
    SemanticOverride {
        canonical: "agent",
        harness: "codex",
        native: "spawn_agent",
    },
    SemanticOverride {
        canonical: "ask_user",
        harness: "codex",
        native: "request_user_input",
    },
    SemanticOverride {
        canonical: "plan_mode",
        harness: "codex",
        native: "update_plan",
    },
    SemanticOverride {
        canonical: "read",
        harness: "opencode",
        native: "view",
    },
    SemanticOverride {
        canonical: "web_search",
        harness: "opencode",
        native: "browser",
    },
    SemanticOverride {
        canonical: "web_fetch",
        harness: "opencode",
        native: "fetch",
    },
    // Cursor — PascalCase convention covers most, but these names diverge
    SemanticOverride {
        canonical: "bash",
        harness: "cursor",
        native: "Shell",
    },
    SemanticOverride {
        canonical: "edit",
        harness: "cursor",
        native: "StrReplace",
    },
    SemanticOverride {
        canonical: "agent",
        harness: "cursor",
        native: "Task",
    },
    SemanticOverride {
        canonical: "ask_user",
        harness: "cursor",
        native: "AskQuestion",
    },
    SemanticOverride {
        canonical: "plan_mode",
        harness: "cursor",
        native: "SwitchMode",
    },
    SemanticOverride {
        canonical: "notebook",
        harness: "cursor",
        native: "EditNotebook",
    },
    // Pi — lowercase convention covers most, but glob is called find
    SemanticOverride {
        canonical: "glob",
        harness: "pi",
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

    if try_parse_mcp_tool_name(trimmed).is_some() {
        return Ok(ParsedToolName {
            name: trimmed.to_string(),
            known: true,
        });
    }

    if head.trim().eq_ignore_ascii_case("mcp")
        && !payload.is_empty()
        && payload.trim().starts_with('(')
    {
        return Err(ToolNameParseError::InvalidMcpRef);
    }

    let canonical = canonicalize_head(head);
    Ok(ParsedToolName {
        name: format!("{}{payload}", canonical.name),
        known: canonical.known,
    })
}

pub(crate) fn project_tool_for_harness(raw: &str, target_harness: &str) -> ProjectedToolName {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ProjectedToolName {
            name: String::new(),
            status: ToolProjectionStatus::UnknownProjected,
        };
    }

    let (head, payload) = split_tool_name(trimmed);
    let head = head.trim();
    if head.is_empty() {
        return ProjectedToolName {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::UnknownProjected,
        };
    }

    if is_mcp_shaped(head) {
        return ProjectedToolName {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::McpVerbatim,
        };
    }

    let canonical = canonicalize_head(head);
    if !canonical.known {
        let harness = target_harness.trim().to_ascii_lowercase();
        let native = match convention_for_harness(&harness) {
            NamingConvention::PascalCase => snake_to_pascal(&canonical.name),
            NamingConvention::SnakeCase => canonical.name.clone(),
            NamingConvention::Lowercase => canonical.name.clone(),
        };
        return ProjectedToolName {
            name: format!("{native}{payload}"),
            status: ToolProjectionStatus::UnknownProjected,
        };
    }

    let harness = target_harness.trim().to_ascii_lowercase();
    let native = semantic_override(canonical.name.as_str(), &harness)
        .map(str::to_string)
        .unwrap_or_else(|| match convention_for_harness(&harness) {
            NamingConvention::PascalCase => snake_to_pascal(&canonical.name),
            NamingConvention::SnakeCase => canonical.name.clone(),
            NamingConvention::Lowercase => strip_underscores(&canonical.name),
        });

    ProjectedToolName {
        name: format!("{native}{payload}"),
        status: ToolProjectionStatus::Known,
    }
}

fn convention_for_harness(harness: &str) -> NamingConvention {
    match harness.trim().to_ascii_lowercase().as_str() {
        "claude" => NamingConvention::PascalCase,
        "codex" => NamingConvention::SnakeCase,
        "opencode" => NamingConvention::Lowercase,
        "cursor" => NamingConvention::PascalCase,
        "pi" => NamingConvention::Lowercase,
        _ => NamingConvention::PascalCase,
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

fn semantic_override(canonical: &str, harness: &str) -> Option<&'static str> {
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

    fn project(raw: &str, harness: &str) -> ProjectedToolName {
        project_tool_for_harness(raw, harness)
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
            ("web", "web_search", true),
        ];

        for (raw, expected_name, expected_known) in cases {
            let parsed = parse(raw);
            assert_eq!(parsed.name, expected_name, "raw: {raw}");
            assert_eq!(parsed.known, expected_known, "raw: {raw}");
        }
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
        assert_eq!(parsed.name, "mcp__github__CreateIssue");
        assert!(!parsed.known);
    }

    #[test]
    fn parse_recognizes_valid_mcp_scoped_refs() {
        let parsed = parse("mcp(GitHub/CreateIssue)");
        assert_eq!(parsed.name, "mcp(GitHub/CreateIssue)");
        assert!(parsed.known);

        let parsed = parse("mcp(github)");
        assert_eq!(parsed.name, "mcp(github)");
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
    fn invalid_mcp_scoped_refs_do_not_project_as_mcp_verbatim() {
        let cases = [
            ("mcp()", "claude", "Mcp()"),
            ("mcp(/x)", "claude", "Mcp(/x)"),
            ("mcp(x/)", "claude", "Mcp(x/)"),
            ("mcp(a/b/c)", "claude", "Mcp(a/b/c)"),
        ];

        for (raw, harness, expected_name) in cases {
            let projected = project(raw, harness);
            assert_eq!(
                projected.name, expected_name,
                "projected name for {raw}, harness: {harness}"
            );
            assert_eq!(
                projected.status,
                ToolProjectionStatus::UnknownProjected,
                "projected status for {raw}, harness: {harness}"
            );
            assert_ne!(
                projected.status,
                ToolProjectionStatus::McpVerbatim,
                "{raw} must not be treated as MCP verbatim"
            );
        }
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
            assert_eq!(parsed.name, raw, "parse name for {raw}");
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
