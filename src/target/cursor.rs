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

use super::{HookEntry, McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct CursorAdapter;

impl TargetAdapter for CursorAdapter {
    fn name(&self) -> &str {
        ".cursor"
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
                format: SectionFormat::JsonSection("mcpServers"),
            },
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
        match super::hook_file::remove_recorded(
            &target_dir.join("hooks.json"),
            &removal.prior_records,
            ".cursor",
            diag,
        ) {
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
    super::hook_file::write(path, hooks, &[("version", serde_json::json!(1))])
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
                emitted_json: serde_json::json!([{"command":"original"}]).to_string(),
            },
        )]);

        super::super::hook_file::remove_recorded(
            &path,
            &records,
            ".cursor",
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
                emitted_json: serde_json::json!([owned]).to_string(),
            },
        )]);

        super::super::hook_file::remove_recorded(
            &path,
            &records,
            ".cursor",
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )
        .unwrap();

        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, serde_json::json!({"version": 1}));
    }
}
