/// `.codex` target adapter.
///
/// Handles MCP server registration and hook binding for the Codex harness.
///
/// Codex-native lowering:
/// - MCP: writes to `codex_mcp.json` (mcpServers section), env vars as plain names
/// - Hooks: writes to `hooks.json` with Codex command hook entries
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, HookEntry, McpServerEntry, TargetAdapter, hook_command};

#[derive(Debug)]
pub struct CodexAdapter;

impl TargetAdapter for CodexAdapter {
    fn name(&self) -> &str {
        ".codex"
    }

    fn known_hook_events(&self) -> Option<&'static [&'static str]> {
        // https://developers.openai.com/codex/hooks — verified 2026-07-24.
        Some(&[
            "SessionStart",
            // SessionEnd is documented at developers.openai.com/codex/hooks but was
            // runtime-verified non-firing in codex-cli 0.144.4 (2026-07-24). Re-add
            // once verified functional; authors can use `unchecked = true` meanwhile.
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "SubagentStart",
            "SubagentStop",
            "Stop",
        ])
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("codex")
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
        let mut written = Vec::new();

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

        if !mcp_servers.is_empty() {
            let path = write_codex_mcp_json(target_dir, &mcp_servers)?;
            written.push(path);
        }

        if !hooks.is_empty() {
            let path = write_hooks_json(target_dir, &hooks)?;
            written.push(path);
        }

        Ok(written)
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_codex_mcp_entries(entry_keys, target_dir)?;
        remove_codex_hook_entries(entry_keys, target_dir)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Codex MCP — `codex_mcp.json` format
// ---------------------------------------------------------------------------
//
// Codex uses plain environment variable names (no interpolation syntax).
// Format:
// {
//   "mcpServers": {
//     "server-name": {
//       "command": "...",
//       "args": [...],
//       "env": ["ENV_VAR_NAME", ...]   ← list of var names, not map
//     }
//   }
// }

fn write_codex_mcp_json(
    target_dir: &Path,
    servers: &[&McpServerEntry],
) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("codex_mcp.json");

    let mut root: serde_json::Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mcp_obj = root
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{} is not a JSON object", path.display()),
            })
        })?
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

        // Codex env: list of variable names (not a map with values).
        if !server.env.is_empty() {
            let env_list: Vec<serde_json::Value> = server
                .env
                .values()
                .map(|v| serde_json::Value::String(v.clone()))
                .collect();
            entry["env"] = serde_json::Value::Array(env_list);
        }

        mcp_map.insert(server.name.clone(), entry);
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
}

fn remove_codex_mcp_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join("codex_mcp.json");
    if !path.is_file() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(mcp_map) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("mcpServers"))
        .and_then(|v| v.as_object_mut())
    {
        for key in entry_keys {
            if let Some(name) = key.strip_prefix("mcp:") {
                mcp_map.remove(name);
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

// ---------------------------------------------------------------------------
// Codex hooks — `hooks.json` format
// ---------------------------------------------------------------------------
//
// Codex command hook entries.
// {
//   "hooks": {
//     "PreToolUse": [
//       {
//         "matcher": "Bash",
//         "hooks": [
//           { "type": "command", "command": "bash /path/to/script.sh" }
//         ]
//       }
//     ]
//   }
// }

fn write_hooks_json(target_dir: &Path, hooks: &[&HookEntry]) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("hooks.json");

    let mut root: serde_json::Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hooks_section = root
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{} is not a JSON object", path.display()),
            })
        })?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_map = hooks_section.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{}: hooks is not an object", path.display()),
        })
    })?;

    for hook in hooks {
        let native_event = hook.native_event.clone();
        let command_entry = serde_json::json!({
            "type": "command",
            "command": hook_command(&hook.script_path),
        });
        let mut hook_binding = serde_json::json!({ "hooks": [command_entry] });
        if let Some(matcher) = &hook.matcher {
            hook_binding["matcher"] = serde_json::Value::String(matcher.clone());
        }
        let event_hooks = hooks_map
            .entry(native_event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("{}: hooks.{native_event} is not an array", path.display()),
                })
            })?;
        remove_managed_hook_entries(event_hooks, &hook.name);
        event_hooks.push(hook_binding);
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
}

fn remove_managed_hook_entries(bindings: &mut Vec<serde_json::Value>, hook_name: &str) -> bool {
    let mut removed = false;
    bindings.retain_mut(|binding| {
        if let Some(command) = binding.as_str() {
            let is_managed = is_managed_hook_command_for(command, hook_name);
            removed |= is_managed;
            return !is_managed;
        }

        let Some(hooks) = binding.get_mut("hooks").and_then(|v| v.as_array_mut()) else {
            return true;
        };
        let mut removed_from_binding = false;
        hooks.retain(|hook| {
            let is_managed = hook
                .get("command")
                .and_then(|v| v.as_str())
                .map(|command| is_managed_hook_command_for(command, hook_name))
                .unwrap_or(false);
            removed_from_binding |= is_managed;
            !is_managed
        });
        removed |= removed_from_binding;
        !removed_from_binding || !hooks.is_empty()
    });
    removed
}

fn is_managed_hook_command_for(command: &str, hook_name: &str) -> bool {
    let normalized = command.replace('\\', "/").replace("//", "/");
    normalized.contains(&format!("/hooks/{hook_name}/"))
}

fn remove_codex_hook_entries(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let hook_names: Vec<&str> = entry_keys
        .iter()
        .filter_map(|k| {
            let rest = k.strip_prefix("hook:")?;
            let (_, name) = rest.split_once(':')?;
            Some(name)
        })
        .collect();

    if hook_names.is_empty() {
        return Ok(());
    }

    remove_codex_hook_entries_from_file(&hook_names, &target_dir.join("hooks.json"))?;
    // Removal-only residue cleanup. Mars before f80062d wrote managed hooks to
    // `codex_hooks.json`; delete this sweep after the next release.
    remove_codex_hook_entries_from_file(&hook_names, &target_dir.join("codex_hooks.json"))?;
    Ok(())
}

fn remove_codex_hook_entries_from_file(hook_names: &[&str], path: &Path) -> Result<(), MarsError> {
    if !path.is_file() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(path).map_err(MarsError::from)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(hooks_map) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|v| v.as_object_mut())
    {
        hooks_map.retain(|_, value| {
            if let Some(arr) = value.as_array_mut() {
                let mut removed = false;
                for name in hook_names {
                    removed |= remove_managed_hook_entries(arr, name);
                }
                return !removed || !arr.is_empty();
            }
            true
        });
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(path, content.as_bytes())?;
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
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "some-mcp@latest".to_string()],
            env: IndexMap::new(),
        })
    }

    fn make_mcp_entry_with_env(name: &str) -> ConfigEntry {
        let mut env = IndexMap::new();
        env.insert("API_KEY".to_string(), "MY_SECRET".to_string());
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            command: "npx".to_string(),
            args: vec![],
            env,
        })
    }

    fn make_hook_entry(name: &str, native: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            matcher: Some("Bash".to_string()),
            script_path: format!("/hooks/{name}/run.sh"),
            order: 0,
        })
    }

    fn make_hook_entry_with_path(name: &str, native: &str, script_path: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            matcher: None,
            script_path: script_path.to_string(),
            order: 0,
        })
    }

    #[test]
    fn write_mcp_creates_codex_mcp_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_mcp_entry("context7")];
        let written = adapter.write_config_entries(&entries, tmp.path()).unwrap();
        assert_eq!(written.len(), 1);
        assert!(tmp.path().join("codex_mcp.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join("codex_mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["context7"].is_object());
    }

    #[test]
    fn write_mcp_env_as_list_of_var_names() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_mcp_entry_with_env("server")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // Codex: env is a list of variable names, not a map with values.
        assert!(json["mcpServers"]["server"]["env"].is_array());
        let env_arr = json["mcpServers"]["server"]["env"].as_array().unwrap();
        assert!(env_arr.iter().any(|v| v.as_str() == Some("MY_SECRET")));
    }

    #[test]
    fn write_hooks_creates_hooks_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_hook_entry("audit", "PreToolUse")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks[0]["matcher"], "Bash");
        assert_eq!(hooks[0]["hooks"][0]["type"], "command");
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/hooks/audit/")
        );
    }

    #[test]
    fn write_hooks_replaces_existing_managed_hook_with_same_event_and_name() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "PreToolUse",
                    "/old/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "PreToolUse",
                    "/new/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/new/hooks/audit/")
        );
    }

    #[test]
    fn remove_mcp_entries_removes_by_name() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_mcp_entry("to-remove"), make_mcp_entry("to-keep")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        adapter
            .remove_config_entries(&["mcp:to-remove".to_string()], tmp.path())
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["to-remove"].is_null());
        assert!(json["mcpServers"]["to-keep"].is_object());
    }

    #[test]
    fn remove_hook_entries_matches_backslash_commands() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit\\\\run.sh\"" }
                        ]
                    },
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit-extended\\\\run.sh\"" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("audit-extended")
        );
    }

    #[test]
    fn remove_hook_entries_preserves_unmanaged_handler_in_same_binding() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"/pkg/hooks/audit/run.sh\"" },
                            { "type": "command", "command": "bash \"/user/hooks/custom.sh\"" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let bindings = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(bindings.len(), 1);
        let hooks = bindings[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert!(
            hooks[0]["command"]
                .as_str()
                .unwrap()
                .contains("/user/hooks/custom.sh")
        );
    }

    #[test]
    fn remove_hook_entries_prunes_sweep_emptied_events_but_preserves_user_empty_events() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"/pkg/hooks/audit/run.sh\"" }
                        ]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "bash \"/pkg/hooks/audit/run.sh\"" }
                        ]
                    }
                ],
                "UserEmptyEvent": []
            }
        });
        std::fs::write(
            tmp.path().join("hooks.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["hooks"]["PreToolUse"].is_null());
        assert!(json["hooks"]["PostToolUse"].is_null());
        assert_eq!(json["hooks"]["UserEmptyEvent"], serde_json::json!([]));
    }

    #[test]
    fn remove_hook_entries_cleans_real_legacy_codex_hooks_json_only_by_managed_path() {
        let tmp = TempDir::new().unwrap();
        let legacy = serde_json::json!({
            "userSetting": "preserved",
            "hooks": {
                "pre-exec": [
                    "bash \"/cache/pkg/hooks/audit/run.sh\"",
                    "printf user-owned"
                ],
                "post-exec": ["bash \"/cache/pkg/hooks/audit/run.sh\""],
                "user-empty": []
            }
        });
        std::fs::write(
            tmp.path().join("codex_hooks.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("codex_hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["userSetting"], "preserved");
        assert_eq!(
            json["hooks"]["pre-exec"],
            serde_json::json!(["printf user-owned"])
        );
        assert!(json["hooks"]["post-exec"].is_null());
        assert_eq!(json["hooks"]["user-empty"], serde_json::json!([]));
    }

    #[test]
    fn remove_hook_entries_still_cleans_object_bindings() {
        let tmp = TempDir::new().unwrap();
        let hooks = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{
                            "type": "command",
                            "command": "bash \"/cache/pkg/hooks/audit/run.sh\""
                        }]
                    },
                    {
                        "matcher": "user-matcher",
                        "hooks": [{
                            "type": "command",
                            "command": "printf user-owned"
                        }]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("hooks.json"),
            serde_json::to_string_pretty(&hooks).unwrap(),
        )
        .unwrap();

        remove_codex_hook_entries(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("hooks.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let bindings = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["matcher"], "user-matcher");
        assert_eq!(bindings[0]["hooks"][0]["command"], "printf user-owned");
    }
}
