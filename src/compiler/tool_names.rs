//! Mars tool-name grammar and target-native projection.

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
}

impl ToolNameParseError {
    pub fn allowed(&self) -> &'static str {
        TOOL_NAME_ALLOWED
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
    Unknown,
}

struct CanonicalTool {
    name: &'static str,
    aliases: &'static [&'static str],
}

const CANONICAL_TOOLS: &[CanonicalTool] = &[
    CanonicalTool {
        name: "bash",
        aliases: &["shell", "terminal"],
    },
    CanonicalTool {
        name: "read",
        aliases: &["cat", "view", "file_read"],
    },
    CanonicalTool {
        name: "write",
        aliases: &["file_write"],
    },
    CanonicalTool {
        name: "edit",
        aliases: &["sed"],
    },
    CanonicalTool {
        name: "agent",
        aliases: &["subagent"],
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
        aliases: &["websearch"],
    },
    CanonicalTool {
        name: "web_fetch",
        aliases: &["webfetch", "fetch", "curl"],
    },
    CanonicalTool {
        name: "ask_user",
        aliases: &["askuser"],
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
        aliases: &["planmode"],
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
    SemanticOverride {
        canonical: "bash",
        harness: "codex",
        native: "shell",
    },
    SemanticOverride {
        canonical: "read",
        harness: "codex",
        native: "file_read",
    },
    SemanticOverride {
        canonical: "write",
        harness: "codex",
        native: "file_write",
    },
    SemanticOverride {
        canonical: "edit",
        harness: "codex",
        native: "file_write",
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
            status: ToolProjectionStatus::Unknown,
        };
    }

    let (head, payload) = split_tool_name(trimmed);
    let head = head.trim();
    if head.is_empty() {
        return ProjectedToolName {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::Unknown,
        };
    }

    let canonical = canonicalize_head(head);
    if !canonical.known {
        return ProjectedToolName {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::Unknown,
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
        "pi" => NamingConvention::PascalCase,
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

fn canonicalize_head(head: &str) -> CanonicalizedHead {
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
            ("bash", "codex", "shell", ToolProjectionStatus::Known),
            ("bash", "opencode", "bash", ToolProjectionStatus::Known),
            ("read", "opencode", "view", ToolProjectionStatus::Known),
            ("read", "codex", "file_read", ToolProjectionStatus::Known),
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
            ("ask_user", "codex", "ask_user", ToolProjectionStatus::Known),
            ("lsp", "claude", "LSP", ToolProjectionStatus::Known),
            (
                "CustomTool",
                "claude",
                "CustomTool",
                ToolProjectionStatus::Unknown,
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
                "shell(git *)",
                ToolProjectionStatus::Known,
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
    fn projection_accepts_input_aliases_and_pascal_case() {
        assert_eq!(project("Bash", "codex").name, "shell");
        assert_eq!(project("shell", "claude").name, "Bash");
        assert_eq!(project("WebSearch", "opencode").name, "browser");
        assert_eq!(project("BASH(git *)", "codex").name, "shell(git *)");
    }
}
