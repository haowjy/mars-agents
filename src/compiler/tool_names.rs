//! Mars tool-name grammar and target-native projection.

const TOOL_NAME_ALLOWED: &str =
    "non-empty tool name; snake_case is normalized to PascalCase when word boundaries are explicit";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedToolName {
    pub name: String,
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

    if let Some(canonical) = canonical_source_tool(head) {
        return Ok(ParsedToolName {
            name: format!("{canonical}{payload}"),
        });
    }

    if head.contains('_')
        && let Some(canonical) = snake_case_to_pascal(head)
    {
        return Ok(ParsedToolName {
            name: format!("{canonical}{payload}"),
        });
    }

    Ok(ParsedToolName {
        name: trimmed.to_string(),
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
    let canonical = canonical_source_tool(head.trim()).unwrap_or(head.trim());
    match native_tool_name(canonical, target_harness) {
        Some(native) => ProjectedToolName {
            name: format!("{native}{payload}"),
            status: ToolProjectionStatus::Known,
        },
        None => ProjectedToolName {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::Unknown,
        },
    }
}

pub(crate) fn is_first_class_harness(harness: &str) -> bool {
    matches!(
        harness.trim().to_ascii_lowercase().as_str(),
        "claude" | "codex" | "opencode"
    )
}

fn split_tool_name(value: &str) -> (&str, &str) {
    match value.find('(') {
        Some(index) => (&value[..index], &value[index..]),
        None => (value, ""),
    }
}

fn canonical_source_tool(head: &str) -> Option<&'static str> {
    match head {
        "Bash" => Some("Bash"),
        "Read" => Some("Read"),
        "Write" => Some("Write"),
        "Edit" => Some("Edit"),
        "Agent" => Some("Agent"),
        "Glob" => Some("Glob"),
        "Grep" => Some("Grep"),
        "Notebook" => Some("Notebook"),
        "Task" => Some("Task"),
        "WebSearch" => Some("WebSearch"),
        "WebFetch" => Some("WebFetch"),
        "TodoRead" => Some("TodoRead"),
        "TodoWrite" => Some("TodoWrite"),
        "Cron" => Some("Cron"),
        "AskUser" => Some("AskUser"),
        "Notifications" => Some("Notifications"),
        "PlanMode" => Some("PlanMode"),
        "Worktree" => Some("Worktree"),
        "LSP" => Some("LSP"),
        "Monitor" => Some("Monitor"),
        "SendUserFile" => Some("SendUserFile"),
        "ScheduleWakeup" => Some("ScheduleWakeup"),
        "RemoteTrigger" => Some("RemoteTrigger"),
        "ToolSearch" => Some("ToolSearch"),
        _ => None,
    }
}

fn snake_case_to_pascal(head: &str) -> Option<String> {
    let mut out = String::new();
    for part in head.split('_') {
        if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return None;
        }
        let mut chars = part.chars();
        let first = chars.next()?;
        out.push(first.to_ascii_uppercase());
        out.extend(chars);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn native_tool_name(canonical: &str, target_harness: &str) -> Option<&'static str> {
    match target_harness.trim().to_ascii_lowercase().as_str() {
        "claude" => native_claude_tool(canonical),
        "codex" => native_codex_tool(canonical),
        "opencode" => native_opencode_tool(canonical),
        "cursor" | "pi" => native_generic_tool(canonical),
        _ => native_generic_tool(canonical),
    }
}

fn native_claude_tool(canonical: &str) -> Option<&'static str> {
    canonical_source_tool(canonical)
}

fn native_codex_tool(canonical: &str) -> Option<&'static str> {
    match canonical {
        "Bash" => Some("shell"),
        "Read" => Some("file_read"),
        "Write" | "Edit" => Some("file_write"),
        "Agent" => Some("agent"),
        _ => None,
    }
}

fn native_opencode_tool(canonical: &str) -> Option<&'static str> {
    match canonical {
        "Bash" => Some("bash"),
        "Read" => Some("read"),
        "Write" => Some("write"),
        "Edit" => Some("edit"),
        "Agent" => Some("agent"),
        "WebSearch" => Some("browser"),
        "WebFetch" => Some("fetch"),
        _ => None,
    }
}

fn native_generic_tool(canonical: &str) -> Option<&'static str> {
    match canonical {
        "Bash" => Some("Bash"),
        "Read" => Some("Read"),
        "Write" => Some("Write"),
        "Edit" => Some("Edit"),
        "Agent" => Some("Agent"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_and_preserves_payload() {
        let parsed = parse_mars_tool_name("Bash(git reset *)").unwrap();
        assert_eq!(parsed.name, "Bash(git reset *)");
    }

    #[test]
    fn snake_case_aliases_canonicalize() {
        let parsed = parse_mars_tool_name("ask_user").unwrap();
        assert_eq!(parsed.name, "AskUser");
        let custom = parse_mars_tool_name("future_tool(scope)").unwrap();
        assert_eq!(custom.name, "FutureTool(scope)");
    }

    #[test]
    fn unknown_unseparated_names_pass_through() {
        assert_eq!(parse_mars_tool_name("askuser").unwrap().name, "askuser");
        assert_eq!(parse_mars_tool_name("Askuser").unwrap().name, "Askuser");
    }

    #[test]
    fn unknown_pascal_case_names_pass_through_for_future_tools() {
        let parsed = parse_mars_tool_name("CustomTool(scope)").unwrap();
        assert_eq!(parsed.name, "CustomTool(scope)");
    }

    #[test]
    fn target_projection_maps_canonical_to_native() {
        assert_eq!(
            project_tool_for_harness("Bash(git *)", "claude").name,
            "Bash(git *)"
        );
        assert_eq!(
            project_tool_for_harness("Bash(git *)", "codex").name,
            "shell(git *)"
        );
        assert_eq!(
            project_tool_for_harness("WebSearch", "opencode").name,
            "browser"
        );
    }
}
