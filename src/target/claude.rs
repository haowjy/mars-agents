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

use super::{ConfigEntry, HookEntry, HookFragmentMode, McpServerEntry, TargetAdapter};

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

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        Some(HookFragmentMode::MergeJson)
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

    fn mcp_config_file_names(&self) -> &'static [&'static str] {
        &[".mcp.json"]
    }
    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["settings.local.json"]
    }

    fn legacy_hook_config_file_names(&self) -> &'static [&'static str] {
        &["settings.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
        target_dir: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> Result<(), MarsError> {
        remove_owned_claude_hooks(records, target_dir, diag)
    }

    fn remove_config_entries(
        &self,
        entry_keys: &[String],
        target_dir: &Path,
    ) -> Result<(), MarsError> {
        remove_mcp_entries_by_key(entry_keys, target_dir)
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
        super::parse_json_file(&path)?
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

    let mut root = super::parse_json_file(&path)?;

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
        super::append_json_event_entries(hooks_map, &hook.native_event, &hook.entries, &path)?;
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;

    Ok(path)
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

fn remove_owned_claude_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<(), MarsError> {
    remove_owned_claude_hooks_from_file(
        records,
        &target_dir.join("settings.local.json"),
        Some(diag),
    )?;
    // One-release bridge: v0.11.0 command-path emissions and pre-local-settings residue.
    // Delete with the other #130 sweeps after the next release.
    let legacy_records: std::collections::BTreeMap<_, _> = records
        .iter()
        .filter(|(_, record)| record.emitted_json.is_none())
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    if legacy_records.is_empty() {
        return Ok(());
    }
    remove_owned_claude_hooks_from_file(&legacy_records, &target_dir.join("settings.json"), None)
}

fn remove_owned_claude_hooks_from_file(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    path: &Path,
    mut diag: Option<&mut crate::diagnostic::DiagnosticCollector>,
) -> Result<(), MarsError> {
    if !path.is_file() {
        return Ok(());
    }
    let mut root = super::parse_json_file(path)?;
    let mut changed = false;
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
                let update = super::remove_json_event_entries(hooks_map, event, &expected);
                changed |= update.changed;
                if update.missing > 0
                    && let Some(diag) = diag.as_deref_mut()
                {
                    diag.warn(
                        "config-divergence",
                        format!(
                            "config-divergence: managed hook `{name}` diverged in target `.claude` at `{}`; preserving edited config and appending the package entry",
                            path.display()
                        ),
                    );
                }
            } else {
                for (event, value) in hooks_map.iter_mut() {
                    if let Some(bindings) = value.as_array_mut() {
                        let before = bindings.len();
                        remove_managed_hook_bindings(bindings, name);
                        changed |= bindings.len() != before;
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
    if changed
        && root
            .get("hooks")
            .and_then(|v| v.as_object())
            .is_some_and(serde_json::Map::is_empty)
    {
        root.as_object_mut().unwrap().remove("hooks");
    }
    if !changed {
        return Ok(());
    }
    crate::fs::atomic_write(
        path,
        serde_json::to_string_pretty(&root)
            .map_err(|e| {
                MarsError::Config(ConfigError::Invalid {
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
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '/hooks/{name}/run.sh'")} ]}),
            ],
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
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '{script_path}'")} ]}),
            ],
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
    fn write_hooks_appends_opaque_entries_in_call_order() {
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
        assert_eq!(hooks.len(), 2);
        assert!(
            hooks[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/old/hooks/audit/")
        );
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

        let records = std::collections::BTreeMap::from([(
            "hook:tool.pre:audit".to_string(),
            crate::lock::ConfigEntryRecord { emitted_json: None },
        )]);
        remove_owned_claude_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let raw = std::fs::read_to_string(tmp.path().join("settings.local.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let hooks = json["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn divergent_structural_removal_does_not_rewrite_settings() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("settings.local.json");
        let original =
            br#"{"hooks":{"SessionStart":[{"hooks":[{"command":"edited"}]}]},"keep":true}"#;
        std::fs::write(&path, original).unwrap();
        let before_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let records = std::collections::BTreeMap::from([(
            "hook:SessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(
                    serde_json::json!([{"hooks":[{"command":"original"}]}]).to_string(),
                ),
            },
        )]);

        remove_owned_claude_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before_modified
        );
    }

    #[test]
    fn write_hooks_never_name_matches_committed_settings_json() {
        let tmp = TempDir::new().unwrap();

        // Without a legacy lock record, path-like user commands are not evidence
        // of ownership and the committed file must remain byte-for-byte intact.
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
        let before = std::fs::read(tmp.path().join("settings.json")).unwrap();

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

        assert_eq!(
            std::fs::read(tmp.path().join("settings.json")).unwrap(),
            before
        );
    }
}
