//! Splitting a lock by the target each record serves.
//!
//! A sync holds only the records of the targets it manages, in the lock it reads and the lock it
//! builds. The rest travel beside it and are joined back where the file is written, so no step in
//! between can compare, prune or plan against a record outside its targets.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;

use super::{CANONICAL_TARGET_ROOT, LockFile, LockedItemV2, record_local_dependency};
use crate::sync::package_dir::PackageDir;

/// The target a per-target item (`hook/guard@claude`) serves, whole: its `.mars` copy, its
/// outputs under the target, and the record that says the target takes none of it.
fn item_target(key: &str, item: &LockedItemV2) -> Option<String> {
    PackageDir::from_lock_key(item.kind, key).map(|dir| dir.target().to_string())
}

impl LockFile {
    /// `(kept, rest)`: `rest` holds every record that serves a target `keep` refuses, and `kept`
    /// everything else. The canonical store always stays in `kept`. [`LockFile::merge_targets`]
    /// is the inverse.
    pub fn split_targets(mut self, keep: impl Fn(&str) -> bool) -> (LockFile, LockFile) {
        let away = |target: &str| target != CANONICAL_TARGET_ROOT && !keep(target);
        let mut rest = LockFile::empty();
        rest.version = self.version;
        let mut kept_items = IndexMap::new();
        for (key, mut item) in std::mem::take(&mut self.items) {
            if let Some(target) = item_target(&key, &item) {
                if away(&target) {
                    rest.items.insert(key, item);
                } else {
                    kept_items.insert(key, item);
                }
                continue;
            }
            let had_outputs = !item.outputs.is_empty();
            let (gone, stay): (Vec<_>, Vec<_>) = std::mem::take(&mut item.outputs)
                .into_iter()
                .partition(|output| away(&output.target_root));
            if !gone.is_empty() {
                rest.items.insert(
                    key.clone(),
                    LockedItemV2 {
                        outputs: gone,
                        ..item.clone()
                    },
                );
            }
            if !stay.is_empty() || !had_outputs {
                item.outputs = stay;
                kept_items.insert(key, item);
            }
        }
        self.items = kept_items;
        let (gone, stay): (BTreeMap<_, _>, BTreeMap<_, _>) =
            std::mem::take(&mut self.config_entries)
                .into_iter()
                .partition(|(target, _)| away(target));
        self.config_entries = stay;
        rest.config_entries = gone;
        (self, rest)
    }

    pub fn merge_targets(&mut self, rest: LockFile) {
        for (key, item) in rest.items {
            match self.items.get_mut(&key) {
                Some(mine) => mine.outputs.extend(item.outputs),
                None => {
                    self.items.insert(key, item);
                }
            }
        }
        self.items.sort_keys();
        self.config_entries.extend(rest.config_entries);
        record_local_dependency(&self.items, &mut self.dependencies);
        self.dependencies.sort_keys();
    }

    /// The targets this lock holds records for, each once.
    pub fn recorded_targets(&self) -> BTreeSet<String> {
        self.items
            .iter()
            .flat_map(|(key, item)| {
                item_target(key, item)
                    .into_iter()
                    .chain(item.outputs.iter().map(|output| output.target_root.clone()))
            })
            .chain(self.config_entries.keys().cloned())
            .filter(|target| target != CANONICAL_TARGET_ROOT)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::OutputRecord;
    use mars_schema::item::ItemKind;

    fn item(kind: ItemKind, outputs: &[(&str, &str)]) -> LockedItemV2 {
        LockedItemV2 {
            source: "_self".into(),
            kind,
            version: None,
            source_checksum: "sha256:src".into(),
            source_path: None,
            outputs: outputs
                .iter()
                .map(|(root, dest)| {
                    OutputRecord::installed(root.to_string(), (*dest).into(), "sha256:out".into())
                })
                .collect(),
        }
    }

    fn lock() -> LockFile {
        let mut lock = LockFile::empty();
        lock.items.insert(
            "hook/audit@claude".into(),
            item(
                ItemKind::Hook,
                &[(".mars", "hooks/claude/audit"), (".claude", "hooks/audit")],
            ),
        );
        lock.items
            .insert("hook/audit@codex".into(), item(ItemKind::Hook, &[]));
        lock.items.insert(
            "skill/plan".into(),
            item(
                ItemKind::Skill,
                &[(".mars", "skills/plan"), (".claude", "skills/plan")],
            ),
        );
        lock
    }

    #[test]
    fn a_per_target_item_goes_whole_to_the_target_it_serves() {
        let (kept, rest) = lock().split_targets(|target| target == ".claude");
        assert!(kept.items.contains_key("hook/audit@claude"));
        assert_eq!(kept.items["hook/audit@claude"].outputs.len(), 2);
        assert!(rest.items.keys().eq(["hook/audit@codex"]));
        assert_eq!(rest.recorded_targets(), BTreeSet::from([".codex".into()]));
        assert_eq!(kept.items["skill/plan"].outputs.len(), 2);
    }

    #[test]
    fn a_shared_item_splits_by_output_and_merges_back() {
        let whole = lock();
        let (kept, rest) = whole.clone().split_targets(|_| false);
        assert!(kept.items.keys().eq(["skill/plan"]));
        assert_eq!(kept.items["skill/plan"].outputs.len(), 1);
        assert_eq!(rest.items.len(), 3);
        let mut joined = kept;
        joined.merge_targets(rest);
        assert!(joined.items.keys().eq(whole.items.keys()));
        assert_eq!(joined.items["skill/plan"].outputs.len(), 2);
    }
}
