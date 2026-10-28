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
use crate::types::DestPath;
use mars_schema::item::ItemKind;

use super::{HookFile, McpFile, McpLocation, SectionFormat, TargetAdapter};

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

    fn hook_file(&self) -> Option<HookFile> {
        // Hook commands carry machine-local cache paths, so they belong in the
        // gitignored `settings.local.json` rather than the committed `settings.json`.
        Some(HookFile {
            file: "settings.local.json",
            top_level: &[],
            trust_gated: false,
        })
    }
}
