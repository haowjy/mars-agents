/// `.agents` target adapter — Meridian-facing full-fidelity output.
///
/// This is the primary target that Meridian reads at runtime. It emits:
/// - `agents/<name>.md` — full-fidelity agent markdown with frontmatter preserved
/// - `skills/<name>/` — portable skill trees also consumed by external harnesses
///
/// See `design/spec/agents-target-adapter.md` for the V0 contract. The key
/// point: NO field stripping — all agent frontmatter fields needed by Meridian
/// are preserved.
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::TargetAdapter;

#[derive(Debug)]
pub struct AgentsAdapter;

impl TargetAdapter for AgentsAdapter {
    fn name(&self) -> &str {
        ".agents"
    }

    fn supports_agents(&self) -> bool {
        true
    }

    fn supports_skills(&self) -> bool {
        true
    }

    fn supports_config_entries(&self) -> bool {
        false
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        let path = match kind {
            ItemKind::Agent => format!("agents/{name}.md"),
            ItemKind::Skill => format!("skills/{name}"),
            ItemKind::Hook => format!("hooks/{name}"),
            ItemKind::McpServer => format!("mcp/{name}"),
            ItemKind::BootstrapDoc => format!("bootstrap/{name}"),
        };
        Some(DestPath::from(path.as_str()))
    }
}
