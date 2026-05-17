pub struct NormalizedTool {
    pub name: String,
    pub status: ToolProjectionStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolProjectionStatus {
    Canonical,
    Normalized,
    Unknown,
}

pub fn normalize_tool_for_harness(raw: &str, target_harness: &str) -> NormalizedTool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return NormalizedTool {
            name: String::new(),
            status: ToolProjectionStatus::Unknown,
        };
    }

    let (head, payload) = match trimmed.find('(') {
        Some(index) => (&trimmed[..index], &trimmed[index..]),
        None => (trimmed, ""),
    };

    let canonical = canonical_tool_name(head, target_harness);
    match canonical {
        Some(value) => {
            let status = if head == value {
                ToolProjectionStatus::Canonical
            } else {
                ToolProjectionStatus::Normalized
            };
            NormalizedTool {
                name: format!("{value}{payload}"),
                status,
            }
        }
        None => NormalizedTool {
            name: trimmed.to_string(),
            status: ToolProjectionStatus::Unknown,
        },
    }
}

pub fn is_first_class_harness(harness: &str) -> bool {
    matches!(
        harness.trim().to_ascii_lowercase().as_str(),
        "claude" | "codex" | "opencode"
    )
}

fn canonical_tool_name(head: &str, target_harness: &str) -> Option<&'static str> {
    let key = head.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }

    match target_harness.trim().to_ascii_lowercase().as_str() {
        "claude" => canonical_claude_tool(&key),
        "codex" => canonical_codex_tool(&key),
        "opencode" => canonical_opencode_tool(&key),
        "cursor" | "pi" => canonical_generic_tool(&key),
        _ => canonical_generic_tool(&key),
    }
}

fn canonical_claude_tool(key: &str) -> Option<&'static str> {
    match key {
        "bash" | "shell" | "terminal" => Some("Bash"),
        "read" | "cat" | "view" => Some("Read"),
        "write" => Some("Write"),
        "edit" | "sed" => Some("Edit"),
        "agent" | "subagent" => Some("Agent"),
        "glob" | "find" => Some("Glob"),
        "grep" | "search" | "rg" => Some("Grep"),
        "notebook" | "jupyter" => Some("Notebook"),
        "task" | "task_tool" => Some("Task"),
        "web_search" | "websearch" => Some("WebSearch"),
        "web_fetch" | "webfetch" => Some("WebFetch"),
        "todo_read" | "todoread" => Some("TodoRead"),
        "todo_write" | "todowrite" => Some("TodoWrite"),
        _ => None,
    }
}

fn canonical_codex_tool(key: &str) -> Option<&'static str> {
    match key {
        "shell" | "bash" | "terminal" => Some("shell"),
        "file_read" | "read" | "cat" => Some("file_read"),
        "file_write" | "write" | "edit" => Some("file_write"),
        "agent" | "subagent" => Some("agent"),
        _ => None,
    }
}

fn canonical_opencode_tool(key: &str) -> Option<&'static str> {
    match key {
        "bash" | "shell" | "terminal" => Some("bash"),
        "read" | "cat" => Some("read"),
        "write" => Some("write"),
        "edit" => Some("edit"),
        "agent" | "subagent" => Some("agent"),
        "browser" | "web_search" | "websearch" => Some("browser"),
        "fetch" | "web_fetch" | "webfetch" => Some("fetch"),
        _ => None,
    }
}

fn canonical_generic_tool(key: &str) -> Option<&'static str> {
    match key {
        "bash" => Some("Bash"),
        "read" => Some("Read"),
        "write" => Some("Write"),
        "edit" => Some("Edit"),
        "agent" => Some("Agent"),
        _ => None,
    }
}
