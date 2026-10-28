/// `.claude` target adapter.
///
/// Handles MCP server registration in the project-root `.mcp.json` and hook
/// binding in `.claude/settings.local.json`.
///
/// Claude-native lowering:
/// - MCP: writes `<project>/.mcp.json` (mcpServers section)
/// - Hooks: writes to `settings.local.json` (hooks section). Hook commands
///   carry machine-local cache paths, so they belong in the gitignored
///   `settings.local.json` rather than the committed `settings.json`.
/// - Env references: rendered as `${VAR_NAME}` for Claude Desktop config compat
use std::path::Path;

use crate::error::{ConfigError, MarsError};
use crate::types::DestPath;
use mars_schema::item::ItemKind;

use super::{HookEntry, HookFragmentMode, McpFile, McpFormat, McpLocation, TargetAdapter};

#[derive(Debug)]
pub struct ClaudeAdapter;

impl TargetAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        ".claude"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        // https://code.claude.com/docs/en/hooks — verified 2026-07-24.
        Some(HookFragmentMode::MergeJson {
            events: &[
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
            ],
        })
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

    fn write_hook_entries(
        &self,
        write: crate::surface_ownership::retention::ConfigWrite<'_>,
        project_root: &Path,
    ) -> Result<(), MarsError> {
        let (target_dir, hooks) = write.into_parts(project_root);
        if hooks.is_empty() {
            return Ok(());
        }
        write_hooks_settings(&target_dir.join("settings.local.json"), &hooks)
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            // The project-root file: Claude reads `<project>/.mcp.json`, not
            // `.claude/.mcp.json`. Claude asks the user to approve project
            // servers; mars does not.
            at: McpLocation {
                path: ".mcp.json",
                format: McpFormat::JsonSection("mcpServers"),
            },
            // Where mars wrote before. Remove in 0.25.
            legacy: Some(McpLocation {
                path: ".claude/.mcp.json",
                format: McpFormat::JsonSection("mcpServers"),
            }),
            render: super::mcp_file::claude_render,
        })
    }

    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["settings.local.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        remove_owned_claude_hooks(&removal.prior_records, &target_dir, diag)
    }
}

// ---------------------------------------------------------------------------
// Hooks — `settings.local.json` format
// ---------------------------------------------------------------------------

/// Write (or merge) Claude hook bindings into `path`.
///
/// Project `settings.local.json` and plugin `hooks/hooks.json` use the same
/// top-level `hooks` object, so the caller chooses the destination while this
/// writer owns the shared shape and merge behavior.
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
pub(crate) fn write_hooks_settings(path: &Path, hooks: &[HookEntry]) -> Result<(), MarsError> {
    let mut root: serde_json::Value = if path.is_file() {
        super::parse_json_file(path)?
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
        super::append_json_event_entries(hooks_map, &hook.native_event, &hook.entries, path)?;
    }

    let content = serde_json::to_string_pretty(&root).map_err(|e| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("failed to serialize {}: {e}", path.display()),
        })
    })?;
    crate::fs::atomic_write(path, content.as_bytes())?;

    Ok(())
}

fn remove_owned_claude_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> crate::surface_ownership::retention::RemovalReport {
    match remove_owned_claude_hooks_from_file(
        records,
        &target_dir.join("settings.local.json"),
        diag,
    ) {
        Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
        Err(error) => {
            crate::surface_ownership::retention::RemovalReport::failed(error, records.clone())
        }
    }
}

fn remove_owned_claude_hooks_from_file(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    path: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
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
        for (key, record) in records.iter().filter(|(key, _)| key.starts_with("hook:")) {
            let Some((event, name)) = key
                .strip_prefix("hook:")
                .and_then(|rest| rest.split_once(':'))
            else {
                continue;
            };
            let Some(expected) = record
                .emitted_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<Vec<serde_json::Value>>(json).ok())
            else {
                continue;
            };
            let update = super::remove_json_event_entries(hooks_map, event, &expected);
            changed |= update.changed;
            if update.missing > 0 {
                diag.warn(
                    "config-divergence",
                    format!(
                        "config-divergence: managed hook `{name}` diverged in target `.claude` at `{}`; preserving edited config and appending the package entry",
                        path.display()
                    ),
                );
            }
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
    use crate::surface_ownership::retention::WritePermit;
    use tempfile::TempDir;

    fn make_hook_entry(name: &str, _event: &str, native: &str) -> HookEntry {
        HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '/hooks/{name}/run.sh'")} ]}),
            ],
        }
    }

    fn make_hook_entry_with_path(
        name: &str,
        _event: &str,
        native: &str,
        script_path: &str,
    ) -> HookEntry {
        HookEntry {
            name: name.to_string(),
            native_event: native.to_string(),
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '{script_path}'")} ]}),
            ],
        }
    }

    #[test]
    fn write_hooks_creates_settings_local_json() {
        let tmp = TempDir::new().unwrap();
        let adapter = ClaudeAdapter;
        let entries = vec![make_hook_entry("audit", "tool.pre", "PreToolUse")];
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(entries.clone()),
                tmp.path(),
            )
            .unwrap();

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
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(vec![make_hook_entry_with_path(
                    "audit",
                    "tool.pre",
                    "PreToolUse",
                    "/old/hooks/audit/run.sh",
                )]),
                tmp.path(),
            )
            .unwrap();
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(vec![make_hook_entry_with_path(
                    "audit",
                    "tool.pre",
                    "PreToolUse",
                    "/new/hooks/audit/run.sh",
                )]),
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
        adapter
            .write_hook_entries(
                WritePermit::for_test("").bind_hooks(entries.clone()),
                tmp.path(),
            )
            .unwrap();

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
