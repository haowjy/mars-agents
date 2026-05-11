/// `.opencode` target adapter.
///
/// Handles MCP server registration and hook binding for the OpenCode harness.
///
/// OpenCode-native lowering:
/// - MCP: writes to `opencode.json` (`mcp` section), env vars as plain name map
/// - Hooks: writes to `opencode.json` (hooks section with plugin hook format)
use std::path::{Path, PathBuf};

use crate::compiler::mcp::{HeaderValue, McpTransport};
use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, HookEntry, McpServerEntry, TargetAdapter, hook_command};

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

        let hooks: Vec<&HookEntry> = entries
            .iter()
            .filter_map(|e| {
                if let ConfigEntry::Hook(h) = e {
                    Some(h)
                } else {
                    None
                }
            })
            .collect();

        if mcp_servers.is_empty() && hooks.is_empty() {
            return Ok(Vec::new());
        }

        // OpenCode merges both into a single config file.
        let path = write_opencode_config(target_dir, &mcp_servers, &hooks)?;
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
// OpenCode uses a single config file with both MCP and hooks:
// {
//   "mcp": {
//     "server-name": {
//       "type": "local",
//       "command": ["npx", "-y", "server-package"],
//       "environment": { "KEY": "VAR_NAME" }   ← plain var name, no interpolation
//     }
//   },
//   "hooks": {
//     "session:start": ["bash /path/to/script.sh"],
//     "tool:before": [...]
//   }
// }

fn write_opencode_config(
    target_dir: &Path,
    servers: &[&McpServerEntry],
    hooks: &[&HookEntry],
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

    migrate_legacy_mcp_servers(root_obj);

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
            let mut entry = match server.transport {
                McpTransport::Stdio => {
                    let mut command = Vec::with_capacity(server.args.len() + 1);
                    if let Some(command_name) = server.command.as_ref() {
                        command.push(serde_json::Value::String(command_name.clone()));
                    }
                    command.extend(server.args.iter().cloned().map(serde_json::Value::String));
                    serde_json::json!({
                        "type": "local",
                        "command": command,
                    })
                }
                McpTransport::Http => serde_json::json!({
                    "type": "remote",
                    "url": server.url,
                }),
            };

            // OpenCode: env as plain name map (no interpolation)
            if !server.env.is_empty() {
                let env_obj: serde_json::Map<String, serde_json::Value> = server
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                entry["environment"] = serde_json::Value::Object(env_obj);
            }

            if !server.headers.is_empty() {
                let headers_obj: serde_json::Map<String, serde_json::Value> = server
                    .headers
                    .iter()
                    .map(|(k, v)| {
                        let value = match v {
                            HeaderValue::EnvRef(env_ref) => {
                                serde_json::Value::String(env_ref.var_name().to_string())
                            }
                            HeaderValue::Plain(plain) => serde_json::Value::String(plain.clone()),
                        };
                        (k.clone(), value)
                    })
                    .collect();
                entry["headers"] = serde_json::Value::Object(headers_obj);
            }

            mcp_map.insert(server.name.clone(), entry);
        }
    }

    // Hooks
    if !hooks.is_empty() {
        let hooks_obj = root_obj
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        let hooks_map = hooks_obj.as_object_mut().ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{}: hooks is not an object", path.display()),
            })
        })?;

        for hook in hooks {
            let command = hook_command(&hook.script_path);
            let native_event = hook.native_event.clone();
            let event_hooks = hooks_map
                .entry(native_event.clone())
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .ok_or_else(|| {
                    MarsError::Config(crate::error::ConfigError::Invalid {
                        message: format!(
                            "{}: hooks.{native_event} is not an array",
                            path.display()
                        ),
                    })
                })?;
            remove_managed_hook_commands(event_hooks, &hook.name);
            event_hooks.push(serde_json::Value::String(command));
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
    migrate_legacy_mcp_servers(root_obj);

    // Remove MCP entries
    if let Some(mcp_map) = root_obj.get_mut("mcp").and_then(|v| v.as_object_mut()) {
        for key in entry_keys {
            if let Some(name) = key.strip_prefix("mcp:") {
                mcp_map.remove(name);
            }
        }
    }

    // Remove hook entries
    let hook_keys: Vec<(String, &str)> = entry_keys
        .iter()
        .filter_map(|k| {
            let rest = k.strip_prefix("hook:")?;
            let (event, name) = rest.split_once(':')?;
            Some((opencode_hook_event(event)?.to_string(), name))
        })
        .collect();

    if !hook_keys.is_empty()
        && let Some(hooks_map) = root_obj.get_mut("hooks").and_then(|v| v.as_object_mut())
    {
        for (event, name) in &hook_keys {
            if let Some(arr) = hooks_map.get_mut(event).and_then(|v| v.as_array_mut()) {
                arr.retain(|cmd| {
                    let cmd_str = cmd.as_str().unwrap_or("");
                    // Exact path-segment match to avoid partial name collisions.
                    !is_managed_hook_command_for(cmd_str, name)
                });
            }
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

fn migrate_legacy_mcp_servers(root_obj: &mut serde_json::Map<String, serde_json::Value>) {
    if root_obj.contains_key("mcp") {
        return;
    }

    let Some(serde_json::Value::Object(legacy_mcp)) = root_obj.remove("mcpServers") else {
        return;
    };

    let migrated = legacy_mcp
        .iter()
        .map(|(name, entry)| (name.clone(), migrate_legacy_server_entry(entry)))
        .collect();
    root_obj.insert("mcp".to_string(), serde_json::Value::Object(migrated));
}

fn migrate_legacy_server_entry(entry: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = entry.as_object() else {
        return serde_json::json!({
            "type": "local",
            "command": [],
        });
    };

    let mut command = Vec::new();
    if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
        command.push(serde_json::Value::String(cmd.to_string()));
    }
    if let Some(args) = obj.get("args").and_then(|v| v.as_array()) {
        command.extend(
            args.iter()
                .filter_map(|v| v.as_str().map(|s| serde_json::Value::String(s.to_string()))),
        );
    }

    let mut migrated = serde_json::Map::new();
    migrated.insert(
        "type".to_string(),
        serde_json::Value::String("local".to_string()),
    );
    migrated.insert("command".to_string(), serde_json::Value::Array(command));

    if let Some(env_obj) = obj.get("env").and_then(|v| v.as_object()) {
        migrated.insert(
            "environment".to_string(),
            serde_json::Value::Object(env_obj.clone()),
        );
    }

    serde_json::Value::Object(migrated)
}

fn opencode_hook_event(event: &str) -> Option<&'static str> {
    match event {
        "session.start" => Some("session:start"),
        "session.end" => Some("session:end"),
        "tool.pre" => Some("tool:before"),
        "tool.post" => Some("tool:after"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn make_stdio_mcp_entry(name: &str) -> ConfigEntry {
        let mut env = IndexMap::new();
        env.insert("TOKEN".to_string(), "MY_TOKEN".to_string());
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            transport: McpTransport::Stdio,
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            env,
            url: None,
            headers: IndexMap::new(),
        })
    }

    fn make_http_mcp_entry(name: &str) -> ConfigEntry {
        let mut headers = IndexMap::new();
        headers.insert(
            "Authorization".to_string(),
            HeaderValue::EnvRef(crate::compiler::mcp::EnvRef::Env {
                var: "API_TOKEN".to_string(),
            }),
        );
        headers.insert(
            "X-Custom".to_string(),
            HeaderValue::Plain("static-value".to_string()),
        );
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            transport: McpTransport::Http,
            command: None,
            args: vec![],
            env: IndexMap::new(),
            url: Some("https://api.example.com/mcp".to_string()),
            headers,
        })
    }

    fn make_hook_entry_with_path(name: &str, native: &str, script_path: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            event: "tool.pre".to_string(),
            native_event: native.to_string(),
            script_path: script_path.to_string(),
            order: 0,
        })
    }

    #[test]
    fn write_config_entries_merges_mcp_and_hooks_into_single_file() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        let written = adapter
            .write_config_entries(
                &[
                    make_stdio_mcp_entry("local-server"),
                    make_http_mcp_entry("remote-server"),
                    make_hook_entry_with_path("audit", "tool:before", "/hooks/audit/run.sh"),
                ],
                tmp.path(),
            )
            .unwrap();

        assert_eq!(written.len(), 1);
        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let local = &json["mcp"]["local-server"];
        assert_eq!(local["type"], "local");
        assert_eq!(local["command"][0], "node");
        assert_eq!(local["command"][1], "server.js");
        assert_eq!(local["environment"]["TOKEN"], "MY_TOKEN");

        let remote = &json["mcp"]["remote-server"];
        assert_eq!(remote["type"], "remote");
        assert_eq!(remote["url"], "https://api.example.com/mcp");
        assert_eq!(remote["headers"]["Authorization"], "API_TOKEN");
        assert_eq!(remote["headers"]["X-Custom"], "static-value");
        assert!(remote["command"].is_null());

        assert!(json["hooks"]["tool:before"].is_array());
    }

    #[test]
    fn write_hooks_replaces_existing_managed_hook_with_same_event_and_name() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "tool:before",
                    "/old/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "tool:before",
                    "/new/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["tool:before"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(hooks[0].as_str().unwrap().contains("/new/hooks/audit/"));
    }

    #[test]
    fn remove_entries_removes_selected_mcp_and_hook_entries() {
        let tmp = TempDir::new().unwrap();
        let adapter = OpencodeAdapter;
        adapter
            .write_config_entries(
                &[
                    make_stdio_mcp_entry("to-remove"),
                    make_stdio_mcp_entry("to-keep"),
                    make_hook_entry_with_path("audit", "tool:before", "/hooks/audit/run.sh"),
                    make_hook_entry_with_path("audit", "tool:after", "/hooks/audit/run.sh"),
                ],
                tmp.path(),
            )
            .unwrap();

        adapter
            .remove_config_entries(
                &[
                    "mcp:to-remove".to_string(),
                    "hook:tool.pre:audit".to_string(),
                ],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcp"]["to-remove"].is_null());
        assert!(json["mcp"]["to-keep"].is_object());
        assert!(json["hooks"]["tool:before"].as_array().unwrap().is_empty());
        assert_eq!(json["hooks"]["tool:after"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_entries_migrates_legacy_mcp_servers_before_cleanup() {
        let tmp = TempDir::new().unwrap();
        let legacy = serde_json::json!({
            "mcpServers": {
                "to-remove": {
                    "command": "npx",
                    "args": ["-y", "legacy-mcp@latest"]
                },
                "to-keep": {
                    "command": "npx",
                    "args": ["-y", "keep-mcp@latest"]
                }
            }
        });
        std::fs::write(
            tmp.path().join("opencode.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let adapter = OpencodeAdapter;
        adapter
            .remove_config_entries(&["mcp:to-remove".to_string()], tmp.path())
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"].is_null());
        assert!(json["mcp"]["to-remove"].is_null());
        assert!(json["mcp"]["to-keep"].is_object());
    }

    #[test]
    fn write_migrates_legacy_mcp_servers_when_mcp_missing() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "mcpServers": {
                "legacy": {
                    "command": "npx",
                    "args": ["-y", "legacy-mcp@latest"],
                    "env": { "TOKEN": "LEGACY_TOKEN" }
                }
            },
            "hooks": {
                "tool:before": [r#"bash "/hooks/audit/run.sh""#]
            }
        });
        std::fs::write(
            tmp.path().join("opencode.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let adapter = OpencodeAdapter;
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "tool:before",
                    "/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"].is_null());
        assert_eq!(json["mcp"]["legacy"]["type"], "local");
        assert_eq!(json["mcp"]["legacy"]["command"][0], "npx");
        assert_eq!(json["mcp"]["legacy"]["command"][1], "-y");
        assert_eq!(
            json["mcp"]["legacy"]["environment"]["TOKEN"],
            "LEGACY_TOKEN"
        );
    }
}
