use crate::types::DestPath;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;

use super::{KeyedSection, McpFile, SectionFormat, TargetAdapter};

#[derive(Debug)]
pub struct PiAdapter;

impl TargetAdapter for PiAdapter {
    fn harness(&self) -> Option<HarnessId> {
        Some(HarnessId::Pi)
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }

    // Pi reads project MCP servers only from trusted projects.
    fn mcp_file(&self) -> Option<McpFile> {
        Some(McpFile {
            at: KeyedSection {
                path: ".pi/mcp.json",
                format: SectionFormat::JsonSection("mcpServers"),
            },
            render: super::mcp_file::claude_render,
        })
    }
}
