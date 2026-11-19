use crate::types::DestPath;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;

use super::{McpFile, McpLocation, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct ClaudeAdapter;

impl TargetAdapter for ClaudeAdapter {
    fn harness(&self) -> Option<HarnessId> {
        Some(HarnessId::Claude)
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
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
}
