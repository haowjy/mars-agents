/// `.pi` target adapter.
///
/// Handles MCP server registration for the Pi harness.
///
/// Pi-native lowering:
/// - MCP: writes `.pi/mcp.json` (`mcpServers` section) with `${VAR}` env
///   interpolation. Pi reads project MCP servers only from trusted projects.
/// - Hooks: whole-file extensions.
use std::path::PathBuf;

use crate::types::DestPath;
use mars_schema::item::ItemKind;

use super::{HookFragmentMode, McpFile, McpLocation, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct PiAdapter;

impl TargetAdapter for PiAdapter {
    fn name(&self) -> &str {
        ".pi"
    }

    fn hook_fragment_mode(&self) -> Option<HookFragmentMode> {
        Some(HookFragmentMode::File)
    }

    fn hook_file_dest_path(&self, name: &str) -> Option<PathBuf> {
        Some(PathBuf::from(format!("extensions/mars-{name}.ts")))
    }

    fn skill_variant_key(&self) -> Option<&str> {
        Some("pi")
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
                path: ".pi/mcp.json",
                format: SectionFormat::JsonSection("mcpServers"),
            },
            render: super::mcp_file::claude_render,
        })
    }
}
