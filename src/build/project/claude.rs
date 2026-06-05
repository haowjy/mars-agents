use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::build::project::{agent_name, approval, effort, model, prompt_file, subprocess_actions};
use crate::error::MarsError;

const CLAUDE_PARENT_ALLOWED_TOOLS_FLAG: &str = "--meridian-parent-allowed-tools";
const CLAUDE_BUILTIN_AGENT_DENY_TOOLS: &[&str] = &[
    "Agent(Explore)",
    "Agent(Plan)",
    "Agent(General-purpose)",
    "Agent(general-purpose)",
];

pub fn project(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    let mut argv = vec![
        "claude".to_string(),
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];

    if !context.interactive {
        argv.push("-".to_string());
    }

    if let Some(model) = model(bundle) {
        argv.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = effort(bundle) {
        argv.extend(["--effort".to_string(), claude_effort(effort).to_string()]);
    }
    if let Some(agent) = agent_name(bundle) {
        argv.extend(["--agent".to_string(), agent.to_string()]);
    }

    let (passthrough_tail, parent_allowed_tools) =
        split_internal_parent_allowed_tools(&context.extra_args);
    let (passthrough_tail, passthrough_allowed, passthrough_disallowed) =
        extract_claude_tool_flags(&passthrough_tail);

    argv.extend(permission_tail(bundle)?);

    let mut allowed_tools = Vec::new();
    allowed_tools.extend(bundle.tools.allowed.iter().cloned());
    allowed_tools.extend(parent_allowed_tools);
    allowed_tools.extend(passthrough_allowed);

    let mut disallowed_tools = Vec::new();
    disallowed_tools.extend(bundle.tools.disallowed.iter().cloned());
    disallowed_tools.extend(passthrough_disallowed);
    disallowed_tools.extend(
        CLAUDE_BUILTIN_AGENT_DENY_TOOLS
            .iter()
            .map(|tool| (*tool).to_string()),
    );

    let mut allowed_tools = dedupe_nonempty(allowed_tools);
    allowed_tools.retain(|tool| !is_claude_agent_tool(tool));
    let disallowed_tools = dedupe_nonempty(disallowed_tools);
    if !disallowed_tools.is_empty() {
        allowed_tools.retain(|tool| !is_denied_tool(tool, &disallowed_tools));
    }

    if !allowed_tools.is_empty() {
        argv.extend(["--allowedTools".to_string(), allowed_tools.join(",")]);
    }
    if !disallowed_tools.is_empty() {
        argv.extend(["--disallowedTools".to_string(), disallowed_tools.join(",")]);
    }

    for tool in &bundle.tools.mcp {
        let normalized = tool.trim();
        if !normalized.is_empty() {
            argv.extend(["--mcp-config".to_string(), normalized.to_string()]);
        }
    }

    let mut files = Vec::new();
    let system_prompt = bundle.prompt_surface.system_instruction.trim();
    if !system_prompt.is_empty() {
        let file = prompt_file(context, bundle.prompt_surface.system_instruction.clone())?;
        argv.extend(["--append-system-prompt-file".to_string(), file.path.clone()]);
        files.push(file);
    }

    if let Some(session_id) = context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        argv.extend(["--resume".to_string(), session_id.to_string()]);
        if context.fork {
            argv.push("--fork-session".to_string());
        }
    }

    for root in &context.workspace_roots {
        argv.extend(["--add-dir".to_string(), root.to_string()]);
    }

    argv.extend(passthrough_tail);

    subprocess_actions(context, argv, files, context.prompt.clone())
}

fn claude_effort(effort: &str) -> &str {
    match effort {
        "xhigh" => "max",
        other => other,
    }
}

fn permission_tail(bundle: &LaunchBundle) -> Result<Vec<String>, MarsError> {
    match approval(bundle) {
        "yolo" | "never" => Ok(vec!["--dangerously-skip-permissions".to_string()]),
        "auto" => Ok(vec![
            "--permission-mode".to_string(),
            "acceptEdits".to_string(),
        ]),
        "confirm" => Ok(vec!["--permission-mode".to_string(), "default".to_string()]),
        "default" => Ok(Vec::new()),
        other => Err(MarsError::InvalidRequest {
            message: format!("Claude projection does not support approval mode '{other}'."),
        }),
    }
}

fn split_internal_parent_allowed_tools(extra_args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut passthrough_tail = Vec::new();
    let mut parent_allowed_tools = Vec::new();
    let mut index = 0;
    while index < extra_args.len() {
        let token = &extra_args[index];
        if token == CLAUDE_PARENT_ALLOWED_TOOLS_FLAG {
            if index + 1 < extra_args.len() {
                parent_allowed_tools.extend(split_csv_entries(&extra_args[index + 1]));
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix(&format!("{CLAUDE_PARENT_ALLOWED_TOOLS_FLAG}=")) {
            parent_allowed_tools.extend(split_csv_entries(value));
            index += 1;
            continue;
        }
        passthrough_tail.push(token.clone());
        index += 1;
    }
    (passthrough_tail, dedupe_nonempty(parent_allowed_tools))
}

fn extract_claude_tool_flags(flags: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut projected = Vec::new();
    let mut allowed = Vec::new();
    let mut disallowed = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        let token = &flags[index];
        if token == "--allowedTools" {
            if index + 1 < flags.len() {
                allowed.extend(split_csv_entries(&flags[index + 1]));
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--allowedTools=") {
            allowed.extend(split_csv_entries(value));
            index += 1;
            continue;
        }
        if token == "--disallowedTools" {
            if index + 1 < flags.len() {
                disallowed.extend(split_csv_entries(&flags[index + 1]));
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--disallowedTools=") {
            disallowed.extend(split_csv_entries(value));
            index += 1;
            continue;
        }
        projected.push(token.clone());
        index += 1;
    }
    (
        projected,
        dedupe_nonempty(allowed),
        dedupe_nonempty(disallowed),
    )
}

fn split_csv_entries(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn dedupe_nonempty(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = value.trim();
        if normalized.is_empty() || !seen.insert(normalized.to_string()) {
            continue;
        }
        out.push(normalized.to_string());
    }
    out
}

fn is_claude_agent_tool(tool: &str) -> bool {
    tool == "Agent" || tool.starts_with("Agent(")
}

fn is_denied_tool(tool: &str, denied_tools: &[String]) -> bool {
    denied_tools
        .iter()
        .any(|denied| denied == tool || (denied == "Agent" && is_claude_agent_tool(tool)))
}
