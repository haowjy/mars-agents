/// `.claude` target adapter.
///
/// Handles MCP server registration in `.mcp.json` and hook binding in
/// `settings.local.json` within the `.claude/` target directory.
///
/// Claude-native lowering:
/// - MCP: writes to `.mcp.json` (mcpServers section)
/// - Hooks: writes to `settings.local.json` (hooks section). Hook commands
///   carry machine-local cache paths, so they belong in the gitignored
///   `settings.local.json` rather than the committed `settings.json`.
/// - Env references: rendered as `${VAR_NAME}` for Claude Desktop config compat
use std::path::{Path, PathBuf};

use crate::error::{ConfigError, MarsError};
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{ConfigEntry, HookEntry, McpServerEntry, TargetAdapter, hook_command};

#[derive(Debug)]
pub struct ClaudeAdapter;

impl TargetAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        ".claude"
    }

    fn known_hook_events(&self) -> Option<&'static [&'static str]> {
        // https://code.claude.com/docs/en/hooks — verified 2026-07-24.
        Some(&[
            "SessionStart",
            "Setup",
            "UserPromptSubmit",
            "UserPromptExpansion",
            "PreToolUse",
            "PermissionRequest",
            "PermissionDenied",
            "PostToolUse",
            "PostToolUseFailure",
            "PostToolBatch",
            "SubagentStart",
            "SubagentStop",
            "TaskCreated",
            "TaskCompleted",
            "Stop",
            "StopFailure",
            "TeammateIdle",
            "PreCompact",
            "PostCompact",
            "Elicitation",
            "ElicitationResult",
            "Notification",
            "ConfigChange",
            "InstructionsLoaded",
            "CwdChanged",
            "FileChanged",
            "WorktreeCreate",
            "WorktreeRemove",
            "SessionEnd",
        ])
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("claude")
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            // Agent, Hook, McpServer, BootstrapDoc routing is deferred.
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
            let path = write_mcp_json(target_dir, &mcp_servers)?;
            written.push(path);
        }

        if !hooks.is_empty() {
            let path = write_hooks_settings(target_dir, &hooks)?;
            written.push(path);
        }

        Ok(written)
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_mcp_entries_by_key(entry_keys, target_dir)?;
        remove_hook_entries_by_key(entry_keys, target_dir)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MCP JSON — `.mcp.json` format
// ---------------------------------------------------------------------------

/// Write (or merge) MCP servers into `<target_dir>/.mcp.json`.
///
/// The file format is:
/// ```json
/// {
///   "mcpServers": {
///     "server-name": {
///       "command": "npx",
///       "args": [...],
///       "env": { "KEY": "${ENV_VAR}" }
///     }
///   }
/// }
/// ```
///
/// Existing entries with other names are preserved (merge, not replace).
fn write_mcp_json(target_dir: &Path, servers: &[&McpServerEntry]) -> Result<PathBuf, MarsError> {
    let path = target_dir.join(".mcp.json");

    // Load existing config or start fresh.
    let mut root: serde_json::Value = if path.is_file() {
        let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers key exists.
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

        if !server.env.is_empty() {
            let env_obj: serde_json::Map<String, serde_json::Value> = server
                .env
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(format!("${{{v}}}"))))
                .collect();
            entry["env"] = serde_json::Value::Object(env_obj);
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

/// Remove MCP server entries by key from `.mcp.json`.
fn remove_mcp_entries_by_key(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
    let path = target_dir.join(".mcp.json");
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
            // Keys are "mcp:<name>" — strip the prefix.
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
// Hooks — `settings.local.json` format
// ---------------------------------------------------------------------------

/// Write (or merge) hook bindings into `<target_dir>/settings.local.json`.
///
/// Hooks go to `settings.local.json` (gitignored) rather than `settings.json`
/// because hook commands embed machine-local cache paths that change on every
/// sync and every machine.
///
/// Claude hooks live in the `hooks` section:
/// ```json
/// {
///   "hooks": {
///     "PreToolUse": [
///       { "hooks": [{ "type": "command", "command": "bash /path/to/script.sh" }] }
///     ]
///   }
/// }
/// ```
fn write_hooks_settings(target_dir: &Path, hooks: &[&HookEntry]) -> Result<PathBuf, MarsError> {
    let path = target_dir.join("settings.local.json");

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
        let native_event = &hook.native_event;
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
                MarsError::Config(ConfigError::Invalid {
                    message: format!("{}: hooks.{native_event} is not an array", path.display()),
                })
            })?;
        remove_managed_hook_bindings(event_hooks, &hook.name);
        event_hooks.push(hook_binding);
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    // Migrate any stale managed hooks out of the committed settings.json. Users
    // who synced before hooks moved to settings.local.json have leftover entries
    // there with machine-local paths; clean them up so they don't persist.
    let hook_names: Vec<&str> = hooks.iter().map(|h| h.name.as_str()).collect();
    migrate_hooks_from_settings_json(target_dir, &hook_names)?;

    Ok(path)
}

/// Remove mars-managed hook bindings from the committed `settings.json`.
///
/// Hooks now live in `settings.local.json`; this strips any leftover managed
/// bindings (matched by `/hooks/<name>/` in the command path) from
/// `settings.json` so stale machine-local paths don't persist in the committed
/// file. Writes back only when something changed.
fn migrate_hooks_from_settings_json(
    target_dir: &Path,
    hook_names: &[&str],
) -> Result<(), MarsError> {
    let path = target_dir.join("settings.json");
    if !path.is_file() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&path).map_err(MarsError::from)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));

    let mut changed = false;

    if let Some(obj) = root.as_object_mut()
        && let Some(hooks_value) = obj.get_mut("hooks")
        && let Some(hooks_map) = hooks_value.as_object_mut()
    {
        for event_hooks in hooks_map.values_mut() {
            if let Some(arr) = event_hooks.as_array_mut() {
                let before = arr.len();
                for name in hook_names {
                    remove_managed_hook_bindings(arr, name);
                }
                if arr.len() != before {
                    changed = true;
                }
            }
        }

        // Drop empty event arrays, then the hooks section if nothing remains.
        hooks_map.retain(|_, v| !v.as_array().map(|a| a.is_empty()).unwrap_or(false));
        if hooks_map.is_empty() {
            obj.remove("hooks");
            changed = true;
        }
    }

    if changed {
        let content = serde_json::to_string_pretty(&root).map_err(|e| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("failed to serialize {}: {e}", path.display()),
            })
        })?;
        crate::fs::atomic_write(&path, content.as_bytes())?;
    }

    Ok(())
}

fn remove_managed_hook_bindings(bindings: &mut Vec<serde_json::Value>, hook_name: &str) {
    bindings.retain(|binding| {
        let Some(inner_hooks) = binding.get("hooks").and_then(|h| h.as_array()) else {
            return true;
        };
        !inner_hooks.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|cmd| is_managed_hook_command_for(cmd, hook_name))
                .unwrap_or(false)
        })
    });
}

fn is_managed_hook_command_for(command: &str, hook_name: &str) -> bool {
    let normalized = command.replace('\\', "/").replace("//", "/");
    normalized.contains(&format!("/hooks/{hook_name}/"))
}

/// Remove hook entries by key from `settings.local.json`.
///
/// Keys are "hook:<event>:<name>" — we use the native event name to locate
/// the section. Because hooks are additive and the settings file may contain
/// user-owned entries, we only remove entries we wrote (matched by command path).
///
/// We also apply the same removal to the committed `settings.json` so any stale
/// managed bindings left there by an older sync get cleaned up.
fn remove_hook_entries_by_key(entry_keys: &[String], target_dir: &Path) -> Result<(), MarsError> {
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

    remove_hook_names_from_file(&target_dir.join("settings.local.json"), &hook_names)?;
    remove_hook_names_from_file(&target_dir.join("settings.json"), &hook_names)?;

    Ok(())
}

/// Remove the given (event, name) managed hook bindings from a single settings
/// file, if it exists. Conservative — only removes entries whose command path
/// matches a mars-managed hook (`/hooks/<name>/`).
fn remove_hook_names_from_file(path: &Path, hook_names: &[&str]) -> Result<(), MarsError> {
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
        for event_hooks in hooks_map.values_mut() {
            if let Some(arr) = event_hooks.as_array_mut() {
                for name in hook_names {
                    remove_managed_hook_bindings(arr, name);
                }
            }
        }
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

    fn make_mcp_entry_with_env(name: &str, env_key: &str, env_var: &str) -> ConfigEntry {
        let mut env = IndexMap::new();
        env.insert(env_key.to_string(), env_var.to_string());
        ConfigEntry::McpServer(McpServerEntry {
            name: name.to_string(),
            command: "npx".to_string(),
            args: vec![],
            env,
        })
    }

    fn make_hook_entry(name: &str, _event: &str, native: &str) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            matcher: None,
            script_path: format!("/hooks/{name}/run.sh"),
            order: 0,
        })
    }

    fn make_hook_entry_with_path(
        name: &str,
        _event: &str,
        native: &str,
        script_path: &str,
    ) -> ConfigEntry {
        ConfigEntry::Hook(HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            matcher: None,
            script_path: script_path.to_string(),
            order: 0,
        })
    }

    #[test]
    fn write_mcp_creates_mcp_json() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();

        let adapter = ClaudeAdapter;
        let entries = vec![make_mcp_entry("context7")];
        let written = adapter.write_config_entries(&entries, tmp.path()).unwrap();

        assert_eq!(written.len(), 1);
        assert!(tmp.path().join(".mcp.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["context7"].is_object());
        assert_eq!(json["mcpServers"]["context7"]["command"], "npx");
    }

    #[test]
    fn write_mcp_merges_with_existing() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "mcpServers": { "existing-server": { "command": "old" } }
        });
        std::fs::write(
            tmp.path().join(".mcp.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        let adapter = ClaudeAdapter;
        let entries = vec![make_mcp_entry("new-server")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["existing-server"].is_object());
        assert!(json["mcpServers"]["new-server"].is_object());
    }

    #[test]
    fn write_mcp_env_renders_as_interpolation() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        let entries = vec![make_mcp_entry_with_env("server", "API_KEY", "MY_SECRET")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            json["mcpServers"]["server"]["env"]["API_KEY"],
            "${MY_SECRET}"
        );
    }

    #[test]
    fn write_hooks_creates_settings_local_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        let entries = vec![make_hook_entry("audit", "tool.pre", "PreToolUse")];
        let written = adapter.write_config_entries(&entries, tmp.path()).unwrap();

        assert_eq!(written.len(), 1);
        assert!(tmp.path().join("settings.local.json").exists());
        assert!(!tmp.path().join("settings.json").exists());

        let raw = std::fs::read_to_string(tmp.path().join("settings.local.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["hooks"]["PreToolUse"].is_array());
        assert!(!json["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
    }

    #[test]
    fn write_hooks_replaces_existing_managed_hook_with_same_event_and_name() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        adapter
            .write_config_entries(
                &[make_hook_entry_with_path(
                    "audit",
                    "tool.pre",
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
                    "tool.pre",
                    "PreToolUse",
                    "/new/hooks/audit/run.sh",
                )],
                tmp.path(),
            )
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("settings.local.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        let command = hooks[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("/new/hooks/audit/"));
    }

    #[test]
    fn remove_mcp_entries_removes_by_name() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        let entries = vec![make_mcp_entry("context7"), make_mcp_entry("other")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        adapter
            .remove_config_entries(&["mcp:context7".to_string()], tmp.path())
            .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json["mcpServers"]["context7"].is_null());
        assert!(json["mcpServers"]["other"].is_object());
    }

    #[test]
    fn write_mcp_and_hooks_both_written() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        let entries = vec![
            make_mcp_entry("context7"),
            make_hook_entry("audit", "tool.pre", "PreToolUse"),
        ];
        let written = adapter.write_config_entries(&entries, tmp.path()).unwrap();
        assert_eq!(written.len(), 2);
        assert!(tmp.path().join(".mcp.json").exists());
        assert!(tmp.path().join("settings.local.json").exists());
        assert!(!tmp.path().join("settings.json").exists());
    }

    #[test]
    fn remove_hook_entries_matches_backslash_commands() {
        let tmp = TempDir::new().unwrap();
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit\\\\run.sh\"" }
                        ]
                    },
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "bash \"C:\\\\pkg\\\\hooks\\\\audit-extended\\\\run.sh\"" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("settings.local.json"),
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        remove_hook_entries_by_key(&["hook:tool.pre:audit".to_string()], tmp.path()).unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("settings.local.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn write_hooks_migrates_stale_hooks_out_of_settings_json() {
        let tmp = TempDir::new().unwrap();

        // Simulate an older sync that wrote a managed hook into the committed
        // settings.json, alongside a user-owned hook that must be preserved.
        let stale = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "bash /old/cache/hooks/audit/run.sh" }
                        ]
                    },
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo user-owned" }
                        ]
                    }
                ]
            }
        });
        std::fs::write(
            tmp.path().join("settings.json"),
            serde_json::to_string_pretty(&stale).unwrap(),
        )
        .unwrap();

        let adapter = ClaudeAdapter;
        let entries = vec![make_hook_entry("audit", "tool.pre", "PreToolUse")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        // New hook lands in settings.local.json.
        let local_raw = std::fs::read_to_string(tmp.path().join("settings.local.json")).unwrap();
        let local: serde_json::Value = serde_json::from_str(&local_raw).unwrap();
        let local_hooks = local["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(local_hooks.len(), 1);
        assert!(
            local_hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/hooks/audit/")
        );

        // Stale managed hook is gone from settings.json; user-owned hook stays.
        let committed_raw = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
        let committed: serde_json::Value = serde_json::from_str(&committed_raw).unwrap();
        let committed_hooks = committed["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(committed_hooks.len(), 1);
        assert_eq!(committed_hooks[0]["hooks"][0]["command"], "echo user-owned");
    }

    #[test]
    fn write_hooks_drops_empty_hooks_section_from_settings_json() {
        let tmp = TempDir::new().unwrap();

        // Only a managed hook in settings.json — after migration the hooks
        // section should be removed entirely.
        let stale = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "bash /old/cache/hooks/audit/run.sh" }
                        ]
                    }
                ]
            },
            "other": "preserved"
        });
        std::fs::write(
            tmp.path().join("settings.json"),
            serde_json::to_string_pretty(&stale).unwrap(),
        )
        .unwrap();

        let adapter = ClaudeAdapter;
        let entries = vec![make_hook_entry("audit", "tool.pre", "PreToolUse")];
        adapter.write_config_entries(&entries, tmp.path()).unwrap();

        let committed_raw = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
        let committed: serde_json::Value = serde_json::from_str(&committed_raw).unwrap();
        assert!(committed.get("hooks").is_none());
        assert_eq!(committed["other"], "preserved");
    }
}
