/// `.cursor` target adapter.
///
/// Handles MCP server registration for the Cursor IDE.
///
/// Cursor-native lowering:
/// - MCP: writes to `mcp.json` (mcpServers section), env vars as `${env:VAR}` syntax
///
use crate::types::DestPath;
use mars_schema::item::ItemKind;
use mars_schema::mcp_servers::{self, Flavor, McpRender};

use super::{HookPlacement, McpFile, McpLocation, McpServerEntry, SectionFormat, TargetAdapter};

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

    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: McpLocation {
                path: ".cursor/mcp.json",
                format: SectionFormat::JsonSection("mcpServers"),
            },
            render,
        })
    }
    fn hook_placement(&self) -> Option<HookPlacement> {
        Some(HookPlacement::Merge {
            file: "hooks.json",
            top_level: HOOKS_TOP_LEVEL,
        })
    }
}

/// Cursor's `hooks.json` is a flat entry array and requires this version wrapper.
pub(crate) const HOOKS_TOP_LEVEL: &[(&str, i64)] = &[("version", 1)];

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
