use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::{HookEntry, McpServerEntry};
use mars_schema::HarnessId;
use mars_schema::mcp_servers::{Flavor, render};
use mars_schema::plugin_metadata::CLAUDE_ROOT_FILE;

use super::super::spec::{
    Finish, NameRule, PluginHarnessSpec, PluginRoot, RootFile, TreeProfile,
    valid_claude_plugin_name,
};

const NATIVE_MODEL_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];
pub(in crate::build::plugin) static PROFILE: TreeProfile = TreeProfile {
    key: "claude",
    harness: HarnessId::Claude,
    root_file: RootFile::Marketplace {
        file: CLAUDE_ROOT_FILE,
        source_prefix: "./",
        name_rule: NameRule {
            validate: valid_claude_plugin_name,
            message: "Claude marketplace names cannot contain spaces, `@`, `:`, path separators, control characters, or bidirectional formatting characters",
        },
    },
    dropped_agent_fields: &["hooks", "mcpServers", "permissionMode", "initialPrompt"],
    approximate_lowers_to_dropped: false,
    agents: Some(apply_agent_policy),
    native_model_aliases: NATIVE_MODEL_ALIASES,
    plugin_root: PluginRoot::Token("${CLAUDE_PLUGIN_ROOT}"),
    check_hook,
    write_config: write_config_entries,
    finish: Finish::StandardManifest {
        path: ".claude-plugin/plugin.json",
        description: "Claude plugin manifest",
    },
};

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}

fn write_config_entries(
    _profile: &TreeProfile,
    root: &std::path::Path,
    mcp: &[McpServerEntry],
    hooks: &[HookEntry],
    harnesses: &[&'static PluginHarnessSpec],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    if !mcp.is_empty() {
        write_plugin_mcp_json(&root.join(".mcp.json"), mcp, harnesses, diag)?;
    }
    if !hooks.is_empty() {
        crate::target::claude::write_hooks_settings(&root.join("hooks/hooks.json"), hooks)?;
    }
    Ok(())
}

fn write_plugin_mcp_json(
    path: &std::path::Path,
    servers: &[McpServerEntry],
    harnesses: &[&'static PluginHarnessSpec],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut root = serde_json::json!({ "mcpServers": {} });
    let mcp_servers = root["mcpServers"]
        .as_object_mut()
        .expect("new MCP root contains an object");

    for server in servers {
        // The file is shared with Codex, so only a Codex build reports what Codex cannot read.
        let rendered = render(&server.transport, Flavor::Plugin);
        if harnesses.iter().any(|harness| harness.key == "codex") {
            for lost in rendered.lossy {
                diag.warn("mcp-lossy", format!("MCP server `{}`: {lost}", server.name));
            }
        }
        mcp_servers.insert(server.name.clone(), rendered.value);
    }

    crate::fs::atomic_write(
        path,
        &super::pretty_json_bytes(&root, &format!("plugin MCP file {}", path.display()))?,
    )
}

fn apply_agent_policy(
    plugin: &mut mars_schema::frontmatter::Frontmatter,
    source: &mars_schema::frontmatter::Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) {
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

    for &field in PROFILE.dropped_agent_fields {
        if source.get(field).is_some() {
            super::warn_agent_field_dropped(diag, item.id.name.as_str(), field, "Claude");
        }
        plugin.remove(field);
    }
}
