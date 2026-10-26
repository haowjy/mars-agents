use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::{ConfigEntry, McpServerEntry};
use mars_schema::HarnessId;

use super::{StandardTreeOutput, TreeProfile, invalid_item};

const NATIVE_MODEL_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];
pub(super) static PROFILE: TreeProfile = TreeProfile {
    format: &crate::plugin::CLAUDE_TREE_SPEC,
    harness: HarnessId::Claude,
    agents: Some(apply_agent_policy),
    native_model_aliases: NATIVE_MODEL_ALIASES,
    plugin_root_token: Some("${CLAUDE_PLUGIN_ROOT}"),
    check_hook,
    standard_output: Some(StandardTreeOutput {
        manifest_path: ".claude-plugin/plugin.json",
        manifest_description: "Claude plugin manifest",
    }),
    write_config: write_config_entries,
    finish: super::finish_standard_tree,
};

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}

fn write_config_entries(
    _profile: &TreeProfile,
    root: &std::path::Path,
    entries: &[ConfigEntry],
    harnesses: &[&'static crate::plugin::PluginHarnessSpec],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let (mcp, hooks) = super::standard_config_entries(entries);

    if !mcp.is_empty() {
        write_plugin_mcp_json(&root.join(".mcp.json"), &mcp, harnesses, diag)?;
    }
    if !hooks.is_empty() {
        crate::target::claude::write_hooks_settings(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}

fn write_plugin_mcp_json(
    path: &std::path::Path,
    servers: &[&McpServerEntry],
    harnesses: &[&'static crate::plugin::PluginHarnessSpec],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut root = serde_json::json!({ "mcpServers": {} });
    let mcp_servers = root["mcpServers"]
        .as_object_mut()
        .expect("new MCP root contains an object");

    for server in servers {
        let mut entry = serde_json::json!({
            "command": server.command,
            "args": server.args,
        });
        let mut env = serde_json::Map::new();
        let mut env_vars = Vec::new();
        let mut renamed = Vec::new();
        for (key, variable) in &server.env {
            if key == variable {
                env_vars.push(variable.clone());
            } else {
                env.insert(
                    key.clone(),
                    serde_json::Value::String(format!("${{{variable}}}")),
                );
                renamed.push(format!("`{key}` from `{variable}`"));
            }
        }
        if !env.is_empty() {
            entry["env"] = serde_json::Value::Object(env);
        }
        if !env_vars.is_empty() {
            entry["env_vars"] = serde_json::json!(env_vars);
        }
        if !renamed.is_empty() && harnesses.iter().any(|harness| harness.key == "codex") {
            diag.warn(
                "plugin-codex-mcp-env-rename",
                format!(
                    "MCP server `{}` renames {}; Codex 0.164.0 passes these `${{VAR}}` values literally instead of reading the source environment variables",
                    server.name,
                    renamed.join(", ")
                ),
            );
        }
        mcp_servers.insert(server.name.clone(), entry);
    }

    crate::fs::atomic_write(
        path,
        &super::pretty_json_bytes(&root, &format!("plugin MCP file {}", path.display()))?,
    )
}

fn apply_agent_policy(
    lowered: &str,
    source: &mars_schema::frontmatter::Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    let mut plugin = mars_schema::frontmatter::parse(lowered)
        .map_err(|error| invalid_item("agent", item, error.to_string()))?;

    // The universal agent schema does not interpret these Claude-native fields,
    // but Claude plugins do. Preserve only the documented plugin-safe subset.
    for field in [
        "maxTurns",
        "autoCompactWindow",
        "memory",
        "background",
        "omitClaudeMd",
        "isolation",
        "color",
    ] {
        if let Some(value) = source.get(field) {
            plugin.insert(field, value.clone());
        }
    }
    if let Some(value) = plugin.remove("disallowed-tools") {
        plugin.insert("disallowedTools", value);
    }

    for &field in PROFILE.dropped_agent_fields() {
        if source.get(field).is_some() {
            diag.warn(
                "plugin-agent-field-dropped",
                format!(
                    "agent `{}`: `{field}` is ignored by Claude plugin agents and was dropped",
                    item.id.name
                ),
            );
        }
        plugin.remove(field);
    }

    Ok(plugin.render())
}
