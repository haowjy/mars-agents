/// `.claude` target adapter stub.
///
/// Future: Claude-native agent lowering, settings.json config entries,
/// MCP server registration, and hook script writing.
///
/// V0: stub only — no per-target behavior yet.
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::TargetAdapter;

#[derive(Debug)]
pub struct ClaudeAdapter;

impl TargetAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        ".claude"
    }

    fn supports_agents(&self) -> bool {
        // Stub: Claude uses its own native agent format; agent file routing is deferred.
        false
    }

    fn supports_skills(&self) -> bool {
        true
    }

    fn supports_config_entries(&self) -> bool {
        // Stub: settings.json config-entry writing is deferred.
        false
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            // Agent, Hook, McpServer, BootstrapDoc routing is deferred.
            _ => None,
        }
    }
}
