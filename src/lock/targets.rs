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

/// What goes to the other half of a split.
pub(super) struct Away<'a> {
    /// A record that serves this target. The canonical store never goes by target.
    pub target: &'a dyn Fn(&str) -> bool,
    /// Every record of this source, the canonical copy included.
    pub source: &'a dyn Fn(&str) -> bool,
    /// A config entry `(target, key)`, and the `provided_entries` mark on it.
    pub entry: &'a dyn Fn(&str, &str) -> bool,
}

impl LockFile {
    /// `(kept, rest)`: `rest` holds every record that serves a target `keep` refuses, and `kept`
    /// everything else. The canonical store always stays in `kept`. [`LockFile::join`] is the
    /// inverse.
    pub fn split_targets(self, keep: impl Fn(&str) -> bool) -> (LockFile, LockFile) {
        let target = |target: &str| !keep(target);
        self.split_by(&Away {
            target: &target,
            source: &|_| false,
            entry: &|target, _| !keep(target),
        })
    }

    /// The one split: a per-output and per-entry choice of the half a record goes to.
    pub(super) fn split_by(mut self, away: &Away) -> (LockFile, LockFile) {
        let target_away = |target: &str| target != CANONICAL_TARGET_ROOT && (away.target)(target);
        let mut rest = LockFile::empty();
        rest.version = self.version;
        let mut kept_items = IndexMap::new();
        for (key, mut item) in std::mem::take(&mut self.items) {
            let source_away = (away.source)(item.source.as_ref());
            if let Some(target) = item_target(&key, &item) {
                if source_away || target_away(&target) {
                    rest.items.insert(key, item);
                } else {
                    kept_items.insert(key, item);
                }
                continue;
            }
            let had_outputs = !item.outputs.is_empty();
            let (gone, stay): (Vec<_>, Vec<_>) = std::mem::take(&mut item.outputs)
                .into_iter()
                .partition(|output| source_away || target_away(&output.target_root));
            if !gone.is_empty() || (source_away && !had_outputs) {
                rest.items.insert(
                    key.clone(),
                    LockedItemV2 {
                        outputs: gone,
                        ..item.clone()
                    },
                );
            }
            if !source_away && (!stay.is_empty() || !had_outputs) {
                item.outputs = stay;
                kept_items.insert(key, item);
            }
        }
        self.items = kept_items;
        let (gone, stay) = std::mem::take(&mut self.dependencies)
            .into_iter()
            .partition(|(name, _)| (away.source)(name.as_ref()));
        self.dependencies = stay;
        rest.dependencies = gone;
        let entry_away =
            |target: &str, key: &str| target != CANONICAL_TARGET_ROOT && (away.entry)(target, key);
        for (target, records) in std::mem::take(&mut self.config_entries) {
            let (gone, stay): (BTreeMap<_, _>, BTreeMap<_, _>) = records
                .into_iter()
                .partition(|(key, _)| entry_away(&target, key));
            for (side, records) in [(&mut self, stay), (&mut rest, gone)] {
                if !records.is_empty() {
                    side.config_entries.insert(target.clone(), records);
                }
            }
        }
        let (gone, stay) = std::mem::take(&mut self.provided_entries)
            .into_iter()
            .partition(|(target, key)| entry_away(target, key));
        self.provided_entries = stay;
        rest.provided_entries = gone;
        (self, rest)
    }

    /// Joins `rest` into `self` output by output and entry by entry, so a shared skill keeps its
    /// `.mars` output in one half and its other outputs in the other. A record both hold takes
    /// the `rest` value, which describes the file written last; an item's own fields stay
    /// `self`'s when `self` has the item. A record in both halves is what a crash mid-move
    /// leaves, and joining collapses it.
    pub fn join(&mut self, rest: LockFile) {
        for (key, item) in rest.items {
            match self.items.get_mut(&key) {
                Some(mine) => {
                    mine.outputs.retain(|output| {
                        item.outputs.iter().all(|other| {
                            other.target_root != output.target_root
                                || other.dest_path != output.dest_path
                        })
                    });
                    mine.outputs.extend(item.outputs);
                }
                None => {
                    self.items.insert(key, item);
                }
            }
        }
        self.items.sort_keys();
        self.dependencies.extend(rest.dependencies);
        for (target, records) in rest.config_entries {
            self.config_entries
                .entry(target)
                .or_default()
                .extend(records);
        }
        self.provided_entries.extend(rest.provided_entries);
        self.yielded.extend(rest.yielded);
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
        joined.join(rest);
        assert!(joined.items.keys().eq(whole.items.keys()));
        assert_eq!(joined.items["skill/plan"].outputs.len(), 2);
    }
}
