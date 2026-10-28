use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::mcp_file::{braced, insert_headers, partition_env};
use crate::target::{ConfigEntry, McpRender, McpServerEntry};
use mars_schema::HarnessId;
use mars_schema::mcp_server::{HeaderValue, McpTransport};
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
    entries: &[ConfigEntry],
    harnesses: &[&'static PluginHarnessSpec],
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

/// Render one server for a Claude plugin tree, which Claude, Codex and Grok
/// share. A stdio same-name reference is a pass-through list (`env_vars`); a
/// renamed one stays a `${VAR}` interpolation, which Codex passes literally, so
/// it is reported as lossy. A remote server is `type: http` with `${VAR}`
/// headers, which Codex likewise passes literally. This stays here, not in the
/// harness `render`s, because the shared file needs `env_vars` alongside `env`.
fn render_plugin_mcp(server: &McpServerEntry) -> McpRender {
    let (entry, lossy) = match &server.transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut entry = serde_json::json!({ "command": command, "args": args });
            let (forwarded, renamed) = partition_env(server);
            if !renamed.is_empty() {
                entry["env"] = renamed
                    .iter()
                    .map(|(key, variable)| ((*key).to_owned(), braced(variable).into()))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            if !forwarded.is_empty() {
                entry["env_vars"] = serde_json::json!(forwarded);
            }
            let lossy = renamed
                .into_iter()
                .map(|(key, variable)| {
                    format!(
                        "Codex 0.164.0 passes `${{{variable}}}` literally instead of reading `{variable}`, so `{key}` is not set from it."
                    )
                })
                .collect();
            (entry, lossy)
        }
        McpTransport::Http { url, headers } => {
            let mut entry = serde_json::json!({ "type": "http", "url": url });
            insert_headers(&mut entry, headers, braced);
            let lossy = headers
                .iter()
                .filter_map(|(name, value)| match value {
                    HeaderValue::Env { var, .. } => Some(format!(
                        "Codex 0.164.0 passes `${{{var}}}` literally instead of reading `{var}`, so header `{name}` is not set from it."
                    )),
                    HeaderValue::Literal(_) => None,
                })
                .collect();
            (entry, lossy)
        }
    };
    McpRender { value: entry, lossy }
}

fn write_plugin_mcp_json(
    path: &std::path::Path,
    servers: &[&McpServerEntry],
    harnesses: &[&'static PluginHarnessSpec],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut root = serde_json::json!({ "mcpServers": {} });
    let mcp_servers = root["mcpServers"]
        .as_object_mut()
        .expect("new MCP root contains an object");

    for server in servers {
        let rendered = render_plugin_mcp(server);
        if harnesses.iter().any(|harness| harness.key == "codex") {
            for lost in rendered.lossy {
                diag.warn(
                    "plugin-codex-mcp-env-rename",
                    format!("MCP server `{}`: {lost}", server.name),
                );
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
