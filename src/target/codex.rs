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

use super::{ConfigEntry, HookEntry, HookFragmentMode, McpServerEntry, TargetAdapter};

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

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        Some(HookFragmentMode::MergeJson)
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

    fn mcp_config_file_names(&self) -> &'static [&'static str] {
        &["codex_mcp.json"]
    }
    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
    }

    fn legacy_hook_config_file_names(&self) -> &'static [&'static str] {
        &["codex_hooks.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
        target_dir: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> Result<(), MarsError> {
        remove_owned_codex_hooks(records, target_dir, diag)
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_codex_mcp_entries(entry_keys, target_dir)
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
        super::parse_json_file(&path)?
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

    let mut root = super::parse_json_file(&path)?;

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
        super::parse_json_file(&path)?
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
        if hook.entries.is_empty() {
            continue;
        }
        let native_event = hook.native_event.clone();

        let event_hooks = hooks_map
            .entry(native_event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("{}: hooks.{native_event} is not an array", path.display()),
                })
            })?;
        event_hooks.extend(hook.entries.iter().cloned());
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

fn remove_owned_codex_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<(), MarsError> {
    remove_owned_codex_hooks_from_file(records, &target_dir.join("hooks.json"), Some(diag))?;
    // Existing legacy-Codex sweep remains until #130's next-release cleanup.
    let legacy_records: std::collections::BTreeMap<_, _> = records
        .iter()
        .filter(|(_, record)| record.emitted_json.is_none())
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    if legacy_records.is_empty() {
        return Ok(());
    }
    remove_owned_codex_hooks_from_file(&legacy_records, &target_dir.join("codex_hooks.json"), None)
}

fn remove_owned_codex_hooks_from_file(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    path: &Path,
    mut diag: Option<&mut crate::diagnostic::DiagnosticCollector>,
) -> Result<(), MarsError> {
    if !path.is_file() {
        return Ok(());
    }
    let mut root = super::parse_json_file(path)?;
    if let Some(hooks_map) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|v| v.as_object_mut())
    {
        let mut emptied_events = std::collections::BTreeSet::new();
        for (key, record) in records.iter().filter(|(key, _)| key.starts_with("hook:")) {
            let Some((event, name)) = key
                .strip_prefix("hook:")
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            if let Some(expected) = record
                .emitted_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(json).ok())
            {
                let mut missing = expected.len();
                if let Some(bindings) = hooks_map.get_mut(event).and_then(|v| v.as_array_mut()) {
                    let before = bindings.len();
                    for entry in expected {
                        if let Some(index) =
                            bindings.iter().position(|candidate| candidate == &entry)
                        {
                            bindings.remove(index);
                            missing -= 1;
                        }
                    }
                    if before > 0 && bindings.is_empty() {
                        emptied_events.insert(event.to_string());
                    }
                }
                if missing > 0
                    && let Some(diag) = diag.as_deref_mut()
                {
                    diag.warn(
                        "config-divergence",
                        format!(
                            "config-divergence: managed hook `{name}` diverged in target `.codex` at `{}`; preserving edited config and appending the package entry",
                            path.display()
                        ),
                    );
                }
            } else {
                for (event, value) in hooks_map.iter_mut() {
                    if let Some(bindings) = value.as_array_mut() {
                        let before = bindings.len();
                        remove_managed_hook_entries(bindings, name);
                        if before > 0 && bindings.is_empty() {
                            emptied_events.insert(event.clone());
                        }
                    }
                }
            }
        }
        for event in emptied_events {
            hooks_map.remove(&event);
        }
    }
    if root
        .get("hooks")
        .and_then(|v| v.as_object())
        .is_some_and(serde_json::Map::is_empty)
    {
        root.as_object_mut().unwrap().remove("hooks");
    }
    crate::fs::atomic_write(
        path,
        serde_json::to_string_pretty(&root)
            .map_err(|e| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize {}: {e}", path.display()),
                })
            })?
            .as_bytes(),
    )
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
            entries: vec![
                serde_json::json!({"matcher": "Bash", "hooks": [{"type": "command", "command": format!("bash '/hooks/{name}/run.sh'")} ]}),
            ],
        })
    }

    fn make_hook_entry_with_path(name: &str, native: &str, script_path: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '{script_path}'")} ]}),
            ],
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
    fn write_hooks_appends_opaque_entries_in_call_order() {
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
        assert_eq!(hooks.len(), 2);
        assert!(
            hooks[1]["hooks"][0]["command"]
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

        let records = std::collections::BTreeMap::from([(
            "hook:tool.pre:audit".to_string(),
            crate::lock::ConfigEntryRecord { emitted_json: None },
        )]);
        remove_owned_codex_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
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
                .contains("audit-extended")
        );
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

        let records = std::collections::BTreeMap::from([(
            "hook:tool.pre:audit".to_string(),
            crate::lock::ConfigEntryRecord { emitted_json: None },
        )]);
        remove_owned_codex_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

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
}
