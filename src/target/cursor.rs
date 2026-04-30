/// `.cursor` target adapter stub.
///
/// Future: Cursor-native agent lowering and config-entry writing.
///
/// V0: stub only — no per-target behavior yet.
use crate::lock::ItemKind;
use crate::types::DestPath;

use super::TargetAdapter;

#[derive(Debug)]
pub struct CursorAdapter;

impl TargetAdapter for CursorAdapter {
    fn name(&self) -> &str {
        ".cursor"
    }

    fn supports_agents(&self) -> bool {
        false
    }

    fn supports_skills(&self) -> bool {
        true
    }

    fn supports_config_entries(&self) -> bool {
        false
    }

    fn default_dest_path(&self, kind: ItemKind, name: &str) -> Option<DestPath> {
        match kind {
            ItemKind::Skill => Some(DestPath::from(format!("skills/{name}").as_str())),
            _ => None,
        }
    }
}
