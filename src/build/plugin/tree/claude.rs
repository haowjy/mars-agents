use std::path::Path;

use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;

use super::{TreeProfile, WrittenTreeContents, invalid_item};
use crate::build::plugin::PackageMetadata;

const NATIVE_MODEL_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];

pub(super) static PROFILE: TreeProfile = TreeProfile {
    harness: HarnessKind::Claude,
    agents: Some(apply_agent_policy),
    native_model_aliases: NATIVE_MODEL_ALIASES,
    plugin_root_token: "${CLAUDE_PLUGIN_ROOT}",
    write_config: write_config_entries,
    finish: write_manifest,
};

fn apply_agent_policy(
    lowered: &str,
    source: &crate::frontmatter::Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    let mut plugin = crate::frontmatter::parse(lowered)
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

    for field in ["hooks", "mcpServers", "permissionMode", "initialPrompt"] {
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

fn write_manifest(
    root: &Path,
    metadata: &PackageMetadata,
    _contents: &WrittenTreeContents<'_>,
) -> Result<(), MarsError> {
    #[derive(Serialize)]
    struct PluginManifest<'a> {
        name: &'a str,
        version: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        author: Option<&'a super::super::PackageAuthor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        homepage: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        license: Option<&'a str>,
    }

    let manifest = PluginManifest {
        name: &metadata.name,
        version: &metadata.version,
        description: metadata.description.as_deref(),
        author: metadata.author.as_ref(),
        homepage: metadata.homepage.as_deref(),
        license: metadata.license.as_deref(),
    };
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        MarsError::Internal(format!(
            "failed to serialize Claude plugin manifest: {error}"
        ))
    })?;
    bytes.push(b'\n');
    crate::fs::atomic_write(&root.join(".claude-plugin/plugin.json"), &bytes)
}

fn write_config_entries(root: &Path, entries: &[ConfigEntry]) -> Result<(), MarsError> {
    let mcp: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::McpServer(entry) => Some(entry),
            ConfigEntry::Hook(_) => None,
        })
        .collect();
    let hooks: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            ConfigEntry::Hook(entry) => Some(entry),
            ConfigEntry::McpServer(_) => None,
        })
        .collect();

    if !mcp.is_empty() {
        crate::target::claude::write_mcp_json(&root.join(".mcp.json"), &mcp)?;
    }
    if !hooks.is_empty() {
        crate::target::claude::write_hooks_settings(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}
