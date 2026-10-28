/// `.cursor` target adapter.
///
/// Handles MCP server registration for the Cursor IDE.
///
/// Cursor-native lowering:
/// - MCP: writes to `mcp.json` (mcpServers section), env vars as `${env:VAR}` syntax
///
use std::path::Path;

use crate::error::MarsError;
use crate::types::DestPath;
use mars_schema::item::ItemKind;
use mars_schema::mcp_servers::{self, Flavor, McpRender};

use super::{
    HookEntry, HookFragmentMode, McpFile, McpFormat, McpLocation, McpServerEntry, TargetAdapter,
};

#[derive(Debug)]
pub struct CursorAdapter;

impl TargetAdapter for CursorAdapter {
    fn name(&self) -> &str {
        ".cursor"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        // cursor-agent 2026.07.16 installed-binary validator, extracted and
        // verified by p5714 on 2026-07-24; p5695 provided the docs baseline.
        Some(HookFragmentMode::MergeJson {
            events: &[
                "beforeShellExecution",
                "beforeMCPExecution",
                "afterShellExecution",
                "afterMCPExecution",
                "beforeReadFile",
                "afterFileEdit",
                "beforeTabFileRead",
                "afterTabFileEdit",
                "stop",
                "beforeSubmitPrompt",
                "afterAgentResponse",
                "afterAgentThought",
                "sessionStart",
                "sessionEnd",
                "preCompact",
                "subagentStart",
                "subagentStop",
                "preToolUse",
                "postToolUse",
                "postToolUseFailure",
                "workspaceOpen",
            ],
        })
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("cursor")
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
    ) -> Result<(), MarsError> {
        let (target_dir, hooks) = write.into_parts(project_root);
        if hooks.is_empty() {
            return Ok(());
        }
        write_cursor_hooks_json(&target_dir.join("hooks.json"), &hooks)
    }

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: McpLocation {
                path: ".cursor/mcp.json",
                format: McpFormat::JsonSection("mcpServers"),
            },
            // v0.23 wrote its records here without `emitted_json`.
            // Remove in 0.25.
            legacy: Some(McpLocation {
                path: ".cursor/mcp.json",
                format: McpFormat::JsonSection("mcpServers"),
            }),
            render,
        })
    }
    fn hook_config_file_names(&self) -> &'static [&'static str] {
        &["hooks.json"]
    }

    fn remove_owned_hook_entries(
        &self,
        operation: crate::surface_ownership::retention::RemovalOperation<'_>,
        project_root: &Path,
        diag: &mut crate::diagnostic::DiagnosticCollector,
    ) -> crate::surface_ownership::retention::RemovalReport {
        let (target_dir, removal) = operation.into_parts(project_root);
        match remove_owned_cursor_hooks(&removal.prior_records, &target_dir, diag) {
            Ok(()) => crate::surface_ownership::retention::RemovalReport::confirmed(),
            Err(error) => crate::surface_ownership::retention::RemovalReport::failed(
                error,
                removal.prior_records.clone(),
            ),
        }
    }
}

// Cursor uses a flat entry array and requires the version wrapper.
pub(crate) fn write_cursor_hooks_json(path: &Path, hooks: &[HookEntry]) -> Result<(), MarsError> {
    let mut root: serde_json::Value = if path.is_file() {
        super::parse_json_file(path)?
    } else {
        serde_json::json!({})
    };
    let root_object = root.as_object_mut().ok_or_else(|| {
        MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{} is not a JSON object", path.display()),
        })
    })?;
    root_object.insert("version".into(), serde_json::json!(1));
    let hooks_map = root_object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            MarsError::Config(crate::error::ConfigError::Invalid {
                message: format!("{}: hooks is not an object", path.display()),
            })
        })?;
    for hook in hooks {
        super::append_json_event_entries(hooks_map, &hook.native_event, &hook.entries, path)?;
    }
    crate::fs::atomic_write(
        path,
        serde_json::to_string_pretty(&root)
            .map_err(|error| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize {}: {error}", path.display()),
                })
            })?
            .as_bytes(),
    )?;
    Ok(())
}

fn remove_owned_cursor_hooks(
    records: &std::collections::BTreeMap<String, crate::lock::ConfigEntryRecord>,
    target_dir: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<(), MarsError> {
    let path = target_dir.join("hooks.json");
    if !path.is_file() {
        return Ok(());
    }
    let mut root = super::parse_json_file(&path)?;
    let mut changed = false;
    if let Some(hooks_map) = root
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (key, record) in records {
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
                        "config-divergence: managed hook `{name}` diverged in target `.cursor` at `{}`; preserving edited config and appending the package entry",
                        path.display()
                    ),
                );
            }
        }
    }
    if changed
        && root
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .is_some_and(serde_json::Map::is_empty)
    {
        root.as_object_mut().unwrap().remove("hooks");
    }
    if !changed {
        return Ok(());
    }
    crate::fs::atomic_write(
        &path,
        serde_json::to_string_pretty(&root)
            .map_err(|error| {
                MarsError::Config(crate::error::ConfigError::Invalid {
                    message: format!("failed to serialize {}: {error}", path.display()),
                })
            })?
            .as_bytes(),
    )
}

// ---------------------------------------------------------------------------
// Cursor MCP — `mcp.json` entry shape
// ---------------------------------------------------------------------------
//
// Cursor uses `${env:VAR_NAME}` interpolation syntax for env vars.
// {
//   "mcpServers": {
//     "server-name": {
//       "command": "...",
//       "args": [...],
//       "env": { "KEY": "${env:VAR_NAME}" }
//     },
//     "remote-name": {
//       "url": "...",
//       "headers": { "Authorization": "Bearer ${env:VAR_NAME}" }
//     }
//   }
// }

fn render(server: &McpServerEntry) -> McpRender {
    mcp_servers::render(&server.transport, Flavor::Cursor)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn divergent_structural_removal_does_not_rewrite_hooks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let original =
            br#"{"version":1,"hooks":{"sessionStart":[{"command":"edited"}]},"keep":true}"#;
        std::fs::write(&path, original).unwrap();
        let before_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let records = std::collections::BTreeMap::from([(
            "hook:sessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(serde_json::json!([{"command":"original"}]).to_string()),
            },
        )]);

        remove_owned_cursor_hooks(
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
    fn removal_prunes_empty_hooks_object() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let owned = serde_json::json!({"command":"owned"});
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "hooks": {"sessionStart": [owned.clone()]}
            })
            .to_string(),
        )
        .unwrap();
        let records = std::collections::BTreeMap::from([(
            "hook:sessionStart:audit".to_string(),
            crate::lock::ConfigEntryRecord {
                emitted_json: Some(serde_json::json!([owned]).to_string()),
            },
        )]);

        remove_owned_cursor_hooks(
            &records,
            tmp.path(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, serde_json::json!({"version": 1}));
    }
}
