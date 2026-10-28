/// `.codex` target adapter.
///
/// Handles MCP server registration and hook binding for the Codex harness.
///
/// Codex-native lowering:
/// - MCP: edits `[mcp_servers.<name>]` in `.codex/config.toml`, env vars forwarded by name
/// - Hooks: writes to `hooks.json` with Codex command hook entries
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::types::DestPath;
use mars_schema::item::ItemKind;

use super::{
    HookEntry, HookFragmentMode, McpFile, McpFormat, McpLocation, McpRender, McpServerEntry,
    TargetAdapter,
};

#[derive(Debug)]
pub struct CodexAdapter;

impl TargetAdapter for CodexAdapter {
    fn name(&self) -> &str {
        ".codex"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        // https://developers.openai.com/codex/hooks — verified 2026-07-24.
        Some(HookFragmentMode::MergeJson {
            events: &[
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
            ],
        })
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

    fn write_hook_entries(
        &self,
        write: crate::surface_ownership::retention::ConfigWrite<'_>,
        project_root: &Path,
    ) -> Result<Vec<PathBuf>, MarsError> {
        let (target_dir, entries) = write.into_parts(project_root);
        let hooks = super::hooks_of(&entries);
        let mut written = Vec::new();
        if !hooks.is_empty() {
            written.push(write_hooks_json(&target_dir, &hooks)?);
        }
        Ok(written)
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            // Codex reads `[mcp_servers.<name>]` from the project's
            // `.codex/config.toml`, for trusted projects only.
            at: McpLocation {
                path: ".codex/config.toml",
                format: McpFormat::TomlTable("mcp_servers"),
            },
            // Where mars wrote before. Delete after one release.
            legacy: Some(McpLocation {
                path: ".codex/codex_mcp.json",
                format: McpFormat::JsonSection("mcpServers"),
            }),
            render,
        })
    }

    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
    }

    fn legacy_hook_config_file_names(&self) -> &'static [&'static str] {
        &["codex_hooks.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        remove_owned_codex_hooks(&removal.prior_records, &target_dir, diag)
    }
}

// ---------------------------------------------------------------------------
// Codex MCP — `[mcp_servers.<name>]` entry shape
// ---------------------------------------------------------------------------
//
// Codex forwards an environment variable to the server by name, in
// `env_vars = ["VAR"]`. A mapping from a differently named key cannot be
// written, so a renamed reference is reported as lossy and dropped.

fn render(server: &McpServerEntry) -> McpRender {
    let mut entry = serde_json::json!({
        "command": server.command,
        "args": server.args,
    });
    let (forwarded, renamed) = super::mcp_file::partition_env(server);
    if !forwarded.is_empty() {
        entry["env_vars"] = serde_json::json!(forwarded);
    }
    McpRender {
        value: entry,
        lossy: renamed
            .into_iter()
            .map(|(key, variable)| {
                format!(
                    "Codex forwards `{variable}` by name and cannot set it as `{key}`; `{key}` is not set for the server."
                )
            })
            .collect(),
    }
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
) -> crate::surface_ownership::retention::RemovalReport {
    if let Err(error) =
        remove_owned_codex_hooks_from_file(records, &target_dir.join("hooks.json"), Some(diag))
    {
        return crate::surface_ownership::retention::RemovalReport::failed(error, records.clone());
    }
    // Existing legacy-Codex sweep remains until #130's next-release cleanup.
    let legacy_records: std::collections::BTreeMap<_, _> = records
        .iter()
        .filter(|(_, record)| record.emitted_json.is_none())
        .map(|(key, record)| (key.clone(), record.clone()))
        .collect();
    if legacy_records.is_empty() {
        return crate::surface_ownership::retention::RemovalReport::confirmed();
    }
    match remove_owned_codex_hooks_from_file(
        &legacy_records,
        &target_dir.join("codex_hooks.json"),
        None,
    ) {
        Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
        Err(error) => {
            crate::surface_ownership::retention::RemovalReport::failed(error, legacy_records)
        }
    }
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
                            "config-divergence: managed hook `{name}` diverged in target `.codex` at `{}`; preserving edited config and appending the package entry",
                            path.display()
                        ),
                    );
                }
            } else {
                for (event, value) in hooks_map.iter_mut() {
                    if let Some(bindings) = value.as_array_mut() {
                        let before = bindings.len();
                        changed |= remove_managed_hook_entries(bindings, name);
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
    use crate::surface_ownership::retention::{Surface, WritePermit};
    use crate::target::ConfigEntry;

    fn write_permit(entries: &[ConfigEntry]) -> WritePermit<'static> {
        WritePermit::for_test("", entries[0].surface())
    }
    use tempfile::TempDir;

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
    fn write_hooks_creates_hooks_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = CodexAdapter;
        let entries = vec![make_hook_entry("audit", "PreToolUse")];
        adapter
            .write_hook_entries(
                write_permit(&entries)
                    .bind_config_entries(entries.clone())
                    .unwrap(),
                tmp.path(),
            )
            .unwrap();

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
            .write_hook_entries(
                WritePermit::for_test("", Surface::Hook)
                    .bind_config_entries(vec![make_hook_entry_with_path(
                        "audit",
                        "PreToolUse",
                        "/old/hooks/audit/run.sh",
                    )])
                    .unwrap(),
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_hook_entries(
                WritePermit::for_test("", Surface::Hook)
                    .bind_config_entries(vec![make_hook_entry_with_path(
                        "audit",
                        "PreToolUse",
                        "/new/hooks/audit/run.sh",
                    )])
                    .unwrap(),
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
    fn divergent_structural_removal_does_not_rewrite_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
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

        remove_owned_codex_hooks(
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
