/// `.opencode` target adapter.
///
/// Handles MCP server registration for the OpenCode harness.
///
/// OpenCode-native lowering:
/// - MCP: writes to `opencode.json` (`mcp` section), with local command arrays and
///   `{env:VAR}` environment references
///
/// Mars has no OpenCode hook config writer; OpenCode hooks are TypeScript plugins.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::types::DestPath;
use mars_schema::item::ItemKind;
use mars_schema::mcp_server::McpTransport;

use super::{
    HookFragmentMode, McpFile, McpFormat, McpLocation, McpRender, McpServerEntry, TargetAdapter,
};

const MCP_SECTION: &str = "mcp";

/// Render one server as an OpenCode `mcp` entry with `{env:VAR}` references:
/// `local` is a command array, `remote` a URL with headers.
fn render(server: &McpServerEntry) -> McpRender {
    let interpolate = |var: &str| format!("{{env:{var}}}");
    let entry = match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({
                "type": "local",
                "command": std::iter::once(command).chain(args).collect::<Vec<_>>(),
            });
            if !env.is_empty() {
                entry["environment"] = env
                    .iter()
                    .map(|(key, env_ref)| {
                        (
                            key.clone(),
                            serde_json::Value::String(interpolate(env_ref.var_name())),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            entry
        }
        McpTransport::Http { url, headers } => {
            let mut entry = serde_json::json!({ "type": "remote", "url": url });
            super::mcp_file::insert_headers(&mut entry, headers, interpolate);
            entry
        }
    };
    McpRender::exact(entry)
}

/// Warning-only check that a fragment exports a default `{ id, setup }`:
/// `export default …`, `export { x as default }` or `export * as default`.
/// It scans identifier tokens, so a commented-out export satisfies it; the cost
/// of that miss is a warning not shown.
fn has_default_export(content: &str) -> bool {
    let tokens: Vec<&str> = content
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$')))
        .filter(|token| !token.is_empty())
        .collect();
    tokens
        .windows(2)
        .any(|pair| matches!(pair, ["export", "default"] | ["as", "default"]))
}

#[derive(Debug)]
pub struct OpencodeAdapter;

impl TargetAdapter for OpencodeAdapter {
    fn name(&self) -> &str {
        ".opencode"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        Some(HookFragmentMode::File)
    }

    fn hook_file_dest_path(&self, name: &str) -> Option<PathBuf> {
        Some(PathBuf::from(format!("plugins/mars-{name}.ts")))
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("opencode")
    }

    fn hook_fragment_problem(&self, content: &str) -> Option<&'static str> {
        (!has_default_export(content))
            .then_some("has no recognizable default export; add a default `{ id, setup }` export")
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: McpLocation {
                path: ".opencode/opencode.json",
                format: McpFormat::JsonSection(MCP_SECTION),
            },
            // Before the `mcp` section existed, mars wrote `mcpServers`.
            legacy: Some(McpLocation {
                path: ".opencode/opencode.json",
                format: McpFormat::JsonSection("mcpServers"),
            }),
            render,
        })
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn legacy_hook_config_file_names(&self) -> &'static [&'static str] {
        &["opencode.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
        _diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        match remove_hook_residue(&removal.prior_records, &target_dir) {
            Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
            Err(error) => crate::surface_ownership::retention::RemovalReport::failed(
                error,
                removal.prior_records.clone(),
            ),
        }
    }
}

fn remove_managed_hook_commands(commands: &mut Vec<serde_json::Value>, hook_name: &str) {
    commands.retain(|cmd| {
        cmd.as_str()
            .map(|cmd| !is_managed_hook_command_for(cmd, hook_name))
            .unwrap_or(true)
    });
}

fn is_managed_hook_command_for(command: &str, hook_name: &str) -> bool {
    let normalized = command.replace('\\', "/").replace("//", "/");
    normalized.contains(&format!("/hooks/{hook_name}/"))
}

/// Removal-only residue cleanup. Versions before native hook authoring wrote
/// an invalid `hooks` object into `opencode.json`. Delete this sweep after one
/// release.
fn remove_hook_residue(
    prior_records: &BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
) -> Result<(), MarsError> {
    let path = target_dir.join("opencode.json");
    if !path.is_file() {
        return Ok(());
    }

    let mut root = super::parse_json_file(&path)?;

    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => return Ok(()),
    };

    let hook_names: Vec<&str> = prior_records
        .keys()
        .filter_map(|k| {
            let rest = k.strip_prefix("hook:")?;
            let (_, name) = rest.split_once(':')?;
            Some(name)
        })
        .collect();

    if !hook_names.is_empty()
        && let Some(hooks_map) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut())
    {
        for commands in hooks_map
            .values_mut()
            .filter_map(|value| value.as_array_mut())
        {
            for name in &hook_names {
                remove_managed_hook_commands(commands, name);
            }
        }
        hooks_map.retain(|_, value| !value.as_array().is_some_and(Vec::is_empty));
        if hooks_map.is_empty() {
            root_obj.remove("hooks");
        }
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn removal_only_sweep_cleans_fabricated_managed_hooks() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("opencode.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "hooks": {
                    "tool:before": ["bash '/cache/hooks/audit/run.sh'"],
                    "session:start": ["echo user-owned"]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        remove_hook_residue(
            &BTreeMap::from([(
                "hook:tool.pre:audit".to_string(),
                crate::lock::ConfigEntryRecord { emitted_json: None },
            )]),
            tmp.path(),
        )
        .unwrap();

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap(),
        )
        .unwrap();
        assert!(json["hooks"]["tool:before"].is_null());
        assert_eq!(json["hooks"]["session:start"][0], "echo user-owned");
    }
}
