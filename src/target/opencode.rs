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

use serde::Serialize;

use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, HookFragmentMode, McpServerEntry, TargetAdapter};

/// The native MCP entry shared by direct target sync and the OpenCode plugin build.
#[derive(Debug, Serialize)]
pub(crate) struct McpServer {
    #[serde(rename = "type")]
    kind: &'static str,
    command: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
}

impl From<&McpServerEntry> for McpServer {
    fn from(server: &McpServerEntry) -> Self {
        Self {
            kind: "local",
            command: std::iter::once(server.command.clone())
                .chain(server.args.iter().cloned())
                .collect(),
            environment: server
                .env
                .iter()
                .map(|(key, variable)| (key.clone(), format!("{{env:{variable}}}")))
                .collect(),
        }
    }
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

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    fn write_config_entries(
        &self,
        write: crate::surface_ownership::retention::ConfigWrite<'_>,
        project_root: &Path,
    ) -> Result<Vec<PathBuf>, MarsError> {
        let (target_dir, entries) = write.into_parts(project_root);
        let mcp_servers: Vec<&McpServerEntry> = entries
            .iter()
            .filter_map(|e| {
                if let ConfigEntry::McpServer(s) = e {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();

        if mcp_servers.is_empty() {
            return Ok(Vec::new());
        }

        let path = (write_opencode_config)(&target_dir, &mcp_servers)?;
        Ok(vec![path])
    }

    fn mcp_config_file_names(&self) -> &'static [&'static str] {
        &["opencode.json"]
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
        let keys: Vec<String> = removal.prior_records.keys().cloned().collect();
        match remove_opencode_entries(&keys, &target_dir) {
            Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
            Err(error) => crate::surface_ownership::retention::RemovalReport::failed(
                error,
                removal.prior_records.clone(),
            ),
        }
    }

    fn remove_config_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        match remove_opencode_entries(&removal.keys_to_remove, &target_dir) {
            Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
            Err(error) => crate::surface_ownership::retention::RemovalReport::failed(
                error,
                removal.prior_records.clone(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// OpenCode config — `opencode.json` format
// ---------------------------------------------------------------------------
//
// OpenCode uses a single config file for MCP:
// {
//   "mcp": {
//     "server-name": {
//       "type": "local",
//       "command": ["...", "..."],
//       "environment": { "KEY": "{env:VAR_NAME}" }
//     }
//   }
// }

fn write_opencode_config(
    target_dir: &Path,
    servers: &[&McpServerEntry],
) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("opencode.json");

    let mut root: serde_json::Value = if path.is_file() {
        super::parse_json_file(&path)?
    } else {
        serde_json::json!({})
    };

    let root_obj = root.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{} is not a JSON object", path.display()),
        })
    })?;

    // A prior release owned these names under `mcpServers`. Remove only names
    // that are being written now; unrelated user entries remain untouched.
    migrate_legacy_mcp_servers(root_obj, servers);

    // MCP servers
    if !servers.is_empty() {
        let mcp_obj = root_obj
            .entry("mcp")
            .or_insert_with(|| serde_json::json!({}));
        let mcp_map = mcp_obj.as_object_mut().ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{}: mcp is not an object", path.display()),
            })
        })?;

        for server in servers {
            let entry = serde_json::to_value(McpServer::from(*server)).map_err(|error| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize OpenCode MCP server: {error}"),
                })
            })?;
            mcp_map.insert(server.name.clone(), entry);
        }
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
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

fn remove_opencode_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join("opencode.json");
    if !path.is_file() {
        return Ok(());
    }

    let mut root = super::parse_json_file(&path)?;

    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => return Ok(()),
    };

    // Remove MCP entries from both the current and one-release legacy keys.
    for section in ["mcp", "mcpServers"] {
        let Some(mcp_map) = root_obj.get_mut(section).and_then(|v| v.as_object_mut()) else {
            continue;
        };
        let mut changed = false;
        for key in entry_keys {
            if let Some(name) = key.strip_prefix("mcp:") {
                changed |= mcp_map.remove(name).is_some();
            }
        }
        if changed && mcp_map.is_empty() {
            root_obj.remove(section);
        }
    }

    // Removal-only residue cleanup. Versions before native hook authoring wrote
    // an invalid `hooks` object here. Delete this sweep after one release.
    let hook_names: Vec<&str> = entry_keys
        .iter()
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

fn migrate_legacy_mcp_servers(
    root_obj: &mut serde_json::Map<String, serde_json::Value>,
    servers: &[&McpServerEntry],
) {
    let Some(legacy_map) = root_obj
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };

    let mut changed = false;
    for server in servers {
        changed |= legacy_map.remove(&server.name).is_some();
    }
    if changed && legacy_map.is_empty() {
        root_obj.remove("mcpServers");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_ownership::retention::WritePermit;

    fn write_permit(entries: &[ConfigEntry]) -> WritePermit<'static> {
        WritePermit::for_test("", entries[0].surface())
    }
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn make_mcp_entry(name: &str) -> ConfigEntry {
        let mut env = IndexMap::new();
        env.insert("TOKEN".to_string(), "MY_TOKEN".to_string());
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            command: "node".to_string(),
            args: vec![],
            env,
        })
    }

    #[test]
    fn write_config_entries_creates_opencode_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let entries = vec![make_mcp_entry("context7")];
        let written = adapter
            .write_config_entries(
                write_permit(&entries)
                    .bind_config_entries(entries.clone())
                    .unwrap(),
                tmp.path(),
            )
            .unwrap();
        assert_eq!(written.len(), 1);
        assert!(tmp.path().join("opencode.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcp"]["context7"].is_object());
        assert_eq!(json["mcp"]["context7"]["type"], "local");
    }

    #[test]
    fn write_mcp_env_as_plain_name_map() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let entries = vec![make_mcp_entry("server")];
        adapter
            .write_config_entries(
                write_permit(&entries)
                    .bind_config_entries(entries.clone())
                    .unwrap(),
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            json["mcp"]["server"]["environment"]["TOKEN"],
            "{env:MY_TOKEN}"
        );
    }

    #[test]
    fn remove_entries_removes_mcp() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let entries = vec![make_mcp_entry("to-remove"), make_mcp_entry("to-keep")];
        adapter
            .write_config_entries(
                write_permit(&entries)
                    .bind_config_entries(entries.clone())
                    .unwrap(),
                tmp.path(),
            )
            .unwrap();

        remove_opencode_entries(&["mcp:to-remove".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcp"]["to-remove"].is_null());
        assert!(json["mcp"]["to-keep"].is_object());
    }

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

        remove_opencode_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap(),
        )
        .unwrap();
        assert!(json["hooks"]["tool:before"].is_null());
        assert_eq!(json["hooks"]["session:start"][0], "echo user-owned");
    }
}
