use std::path::Path;

use serde::Serialize;

use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;
use crate::target::ConfigEntry;

use super::super::PluginTree;
use super::super::metadata::{PackageAuthor, PackageMetadata};
use super::{TreeProfile, WrittenTreeContents};

pub(super) static PROFILE: TreeProfile = TreeProfile {
    tree: PluginTree::Cursor,
    harness: HarnessKind::Cursor,
    agents: Some(preserve_agent),
    native_model_aliases: &[],
    plugin_root_token: Some("${CURSOR_PLUGIN_ROOT}"),
    check_hook,
    write_config: write_config_entries,
    finish: write_manifest,
};

fn preserve_agent(
    lowered: &str,
    _source: &crate::frontmatter::Frontmatter,
    _item: &TargetItem,
    _diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    Ok(lowered.to_string())
}

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}

fn write_manifest(
    root: &Path,
    metadata: &PackageMetadata,
    _contents: &WrittenTreeContents<'_>,
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    #[derive(Serialize)]
    struct PluginManifest<'a> {
        name: &'a str,
        version: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        author: Option<&'a PackageAuthor>,
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
            "failed to serialize Cursor plugin manifest: {error}"
        ))
    })?;
    bytes.push(b'\n');
    crate::fs::atomic_write(&root.join(".cursor-plugin/plugin.json"), &bytes)
}

fn write_config_entries(
    root: &Path,
    entries: &[ConfigEntry],
    _diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
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
        crate::target::cursor::write_cursor_mcp_json(&root.join("mcp.json"), &mcp)?;
    }
    if !hooks.is_empty() {
        crate::target::cursor::write_cursor_hooks_json(&root.join("hooks/hooks.json"), &hooks)?;
    }
    Ok(())
}
