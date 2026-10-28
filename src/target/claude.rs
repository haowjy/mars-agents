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

use crate::error::MarsError;
use crate::types::DestPath;
use mars_schema::item::ItemKind;

use super::{HookEntry, McpFile, McpLocation, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct ClaudeAdapter;

impl TargetAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        ".claude"
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
                format: SectionFormat::JsonSection("mcpServers"),
            },
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
    super::hook_file::write(path, hooks, &[])
}

fn remove_owned_claude_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> crate::surface_ownership::retention::RemovalReport {
    match super::hook_file::remove_recorded(
        &target_dir.join("settings.local.json"),
        records,
        ".claude",
        diag,
    ) {
        Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
        Err(error) => {
            crate::surface_ownership::retention::RemovalReport::failed(error, records.clone())
        }
    }
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
                emitted_json: serde_json::json!([{"hooks":[{"command":"original"}]}]).to_string(),
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

        // Without a lock record, path-like user commands are not evidence
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
