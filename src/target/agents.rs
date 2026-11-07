use crate::types::DestPath;
/// are preserved.
use mars_schema::item::ItemKind;

use super::TargetAdapter;

// Legacy `.agents` outputs retain full agent frontmatter.
#[derive(Debug)]
pub struct AgentsAdapter;

impl TargetAdapter for AgentsAdapter {
    fn name(&self) -> &str {
        ".agents"
    }

    fn skill_variant_key(&self) -> Option<&str> {
        None
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        let path = match kind {
            ItemKind::Agent => format!("agents/{name}.md"),
            ItemKind::Skill => format!("skills/{name}"),
            ItemKind::BootstrapDoc => format!("bootstrap/{name}/BOOTSTRAP.md"),
            // Hooks and MCP servers install as target-scoped package directories,
            // whose paths `PackageDir` owns.
            ItemKind::Hook | ItemKind::McpServer => return None,
        };
        Some(DestPath::from(path.as_str()))
    }
}
