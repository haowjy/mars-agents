/// `.opencode` target adapter.
///
/// Handles MCP server registration for the OpenCode harness.
///
/// OpenCode-native lowering:
/// - MCP: writes to `opencode.json` (mcpServers section), env vars as plain name map
///
/// Mars has no OpenCode hook writer; OpenCode extensibility uses TypeScript plugins.
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, McpServerEntry, TargetAdapter};

#[derive(Debug)]
pub struct OpencodeAdapter;

impl TargetAdapter for OpencodeAdapter {
    fn name(&self) -> &str {
        ".opencode"
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
        entries: &[ConfigEntry],
        target_dir: &Path,
    ) -> Result<Vec<PathBuf>, MarsError> {
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

        let path = write_opencode_config(target_dir, &mcp_servers)?;
        Ok(vec![path])
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_opencode_entries(entry_keys, target_dir)
    }
}

// ---------------------------------------------------------------------------
// OpenCode config — `opencode.json` format
// ---------------------------------------------------------------------------
//
// OpenCode uses a single config file for MCP:
// {
//   "mcpServers": {
//     "server-name": {
//       "command": "...",
//       "args": [...],
//       "env": { "KEY": "VAR_NAME" }   ← plain var name, no interpolation
//     }
//   }
// }

fn write_opencode_config(
    target_dir: &Path,
    servers: &[&McpServerEntry],
) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("opencode.json");

    let mut root: serde_json::Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let root_obj = root.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{} is not a JSON object", path.display()),
        })
    })?;

    // MCP servers
    if !servers.is_empty() {
        let mcp_obj = root_obj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}));
        let mcp_map = mcp_obj.as_object_mut().ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{}: mcpServers is not an object", path.display()),
            })
        })?;

        for server in servers {
            let mut entry = serde_json::json!({
                "command": server.command,
                "args": server.args,
            });

            // OpenCode: env as plain name map (no interpolation)
            if !server.env.is_empty() {
                let env_obj: serde_json::Map<String, serde_json::Value> = server
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                entry["env"] = serde_json::Value::Object(env_obj);
            }

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

    let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));

    let root_obj = match root.as_object_mut() {
        Some(o) => o,
        None => return Ok(()),
    };

    // Remove MCP entries
    if let Some(mcp_map) = root_obj
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
    {
        for key in entry_keys {
            if let Some(name) = key.strip_prefix("mcp:") {
                mcp_map.remove(name);
            }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
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
        let written = adapter.write_config_entries(&entries, tmp.path()).unwrap();
        assert_eq!(written.len(), 1);
        assert!(tmp.path().join("opencode.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["context7"].is_object());
    }

    #[test]
    fn write_mcp_env_as_plain_name_map() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let entries = vec![make_mcp_entry("server")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // OpenCode: env is a plain name map (not interpolated)
        assert_eq!(json["mcpServers"]["server"]["env"]["TOKEN"], "MY_TOKEN");
    }

    #[test]
    fn remove_entries_removes_mcp() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let entries = vec![make_mcp_entry("to-remove"), make_mcp_entry("to-keep")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        adapter
            .remove_config_entries(&["mcp:to-remove".to_string()], tmp.path())
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["to-remove"].is_null());
        assert!(json["mcpServers"]["to-keep"].is_object());
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
