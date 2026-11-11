//! Tool rules a caller supplies in place of the agent's own (`--agent-tool-rules`).

use mars_schema::tool_names::{parse_mars_tool_name, render_scoped};
use mars_schema::tool_policy::{RuleOrigin, ToolAction, ToolRule};
use serde::Deserialize;

use crate::error::MarsError;

#[derive(Deserialize)]
struct Entry {
    capability: String,
    #[serde(default)]
    pattern: Option<String>,
    action: ToolAction,
}

/// Parse a JSON array of `tools.rules` entries. Entries whose `origin` is not `profile` or
/// `overlay` are skipped; one entry that does not parse fails the whole input.
pub(crate) fn parse_agent_tool_rules(json: &str) -> Result<Vec<ToolRule>, MarsError> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|err| {
        invalid(format!(
            "--agent-tool-rules must be a JSON array of rule objects: {err}"
        ))
    })?;
    let mut rules = Vec::new();
    for (index, value) in entries.into_iter().enumerate() {
        let fail = |reason: String| invalid(format!("--agent-tool-rules entry {index}: {reason}"));
        let origin = match value.get("origin").and_then(|origin| origin.as_str()) {
            Some("profile") => RuleOrigin::Profile,
            Some("overlay") => RuleOrigin::Overlay,
            Some(_) => continue,
            None => return Err(fail("`origin` must be a string".to_string())),
        };
        let entry = Entry::deserialize(value).map_err(|err| fail(err.to_string()))?;
        // The wire form: `*` and `mcp` carry their scope in `capability` / `pattern`.
        let raw = match entry.capability.as_str() {
            "mcp" => format!("mcp({})", entry.pattern.as_deref().unwrap_or("")),
            name => render_scoped(name, entry.pattern.as_deref()),
        };
        let parsed = parse_mars_tool_name(&raw)
            .map_err(|_| fail(format!("`{raw}` is not a tool name Mars accepts")))?;
        rules.push(ToolRule::from_parsed(parsed, entry.action).with_origin(origin));
    }
    Ok(rules)
}

fn invalid(message: String) -> MarsError {
    MarsError::InvalidRequest { message }
}
