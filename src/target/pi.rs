/// `.pi` target adapter stub.
///
/// Future: Pi-native agent lowering and config-entry writing.
///
/// V0: stub only — no per-target behavior yet.
use std::path::PathBuf;

use crate::lock::ItemKind;
use crate::types::DestPath;

use super::{HookFragmentMode, TargetAdapter};

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
}
