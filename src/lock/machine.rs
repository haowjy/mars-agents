//! Which records of a lock live in the machine-local `.mars/machine.lock` and which in the
//! committed `mars.lock`, and how the two are joined back.
//!
//! A record is machine-local when it describes one machine: provided content (`mars provide`),
//! or anything that serves a target `mars.toml` does not name, because only this machine's
//! `mars.local.toml` asked for it. Everything else is shared. [`split`] and [`join`] are
//! inverses over the lock a sync sees.

use std::collections::BTreeSet;

use super::{LockFile, LockedItemV2, record_local_dependency};
use crate::provided::is_provided_source;
use crate::types::SourceOrigin;

/// `(committed, machine-local)`. `project_targets` are the targets `mars.toml` names.
pub(super) fn split(lock: LockFile, project_targets: &BTreeSet<String>) -> (LockFile, LockFile) {
    let (committed, mut local) = lock.split_targets(|target| project_targets.contains(target));
    let (mut committed, provided) = split_provided(committed);
    local.join(provided);
    // `provided_entries` marks entries in the file that holds them, so it lives only there.
    local
        .provided_entries
        .extend(std::mem::take(&mut committed.provided_entries));
    // `_self` names the project's own package in the file whose items use it.
    for lock in [&mut committed, &mut local] {
        let local_source: crate::types::SourceName = SourceOrigin::LocalPackage.to_string().into();
        lock.dependencies.shift_remove(&local_source);
        record_local_dependency(&lock.items, &mut lock.dependencies);
    }
    (committed, local)
}

fn split_provided(mut committed: LockFile) -> (LockFile, LockFile) {
    let mut provided = LockFile::empty();
    provided.version = committed.version;
    committed.items.retain(|key, item| {
        let is_provided = is_provided_source(item.source.as_ref());
        if is_provided {
            provided.items.insert(key.clone(), item.clone());
        }
        !is_provided
    });
    committed.dependencies.retain(|name, dependency| {
        let is_provided = is_provided_source(name.as_ref());
        if is_provided {
            provided
                .dependencies
                .insert(name.clone(), dependency.clone());
        }
        !is_provided
    });
    for (target, key) in &committed.provided_entries {
        if let Some(record) = committed
            .config_entries
            .get_mut(target)
            .and_then(|records| records.remove(key))
        {
            provided
                .config_entries
                .entry(target.clone())
                .or_default()
                .insert(key.clone(), record);
        }
    }
    committed
        .config_entries
        .retain(|_, records| !records.is_empty());
    (committed, provided)
}

impl LockFile {
    /// Joins `local` into `self` output by output and entry by entry, so a shared skill keeps its
    /// `.mars` output here and its local-only outputs there. A record both hold takes the
    /// `local` value, which describes this machine's disk; an item's own fields stay `self`'s
    /// when `self` has the item.
    pub(super) fn join(&mut self, local: LockFile) {
        for (key, item) in local.items {
            match self.items.get_mut(&key) {
                Some(mine) => merge_outputs(mine, item),
                None => {
                    self.items.insert(key, item);
                }
            }
        }
        self.items.sort_keys();
        self.dependencies.extend(local.dependencies);
        self.dependencies.sort_keys();
        for (target, records) in local.config_entries {
            self.config_entries
                .entry(target)
                .or_default()
                .extend(records);
        }
        self.provided_entries.extend(local.provided_entries);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.dependencies.is_empty() && self.config_entries.is_empty()
    }
}

fn merge_outputs(mine: &mut LockedItemV2, theirs: LockedItemV2) {
    mine.outputs.retain(|output| {
        theirs.outputs.iter().all(|other| {
            other.target_root != output.target_root || other.dest_path != output.dest_path
        })
    });
    mine.outputs.extend(theirs.outputs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{CANONICAL_TARGET_ROOT, ConfigEntryRecord, OutputRecord};
    use mars_schema::item::ItemKind;

    fn item(source: &str, kind: ItemKind, outputs: &[(&str, &str, &str)]) -> LockedItemV2 {
        LockedItemV2 {
            source: source.into(),
            kind,
            version: None,
            source_checksum: "sha256:src".into(),
            source_path: None,
            outputs: outputs
                .iter()
                .map(|(root, dest, sum)| {
                    OutputRecord::installed(root.to_string(), (*dest).into(), (*sum).into())
                })
                .collect(),
        }
    }

    fn project(targets: &[&str]) -> BTreeSet<String> {
        targets.iter().map(|target| target.to_string()).collect()
    }

    fn shared() -> LockFile {
        let mut lock = LockFile::empty();
        lock.items.insert(
            "skill/plan".into(),
            item(
                "_self",
                ItemKind::Skill,
                &[
                    (".mars", "skills/plan", "sha256:a"),
                    (".claude", "skills/plan", "sha256:a"),
                    (".agy", "skills/plan", "sha256:a"),
                ],
            ),
        );
        lock.config_entries
            .entry(".agy".into())
            .or_default()
            .insert(
                "mcp:srv".into(),
                ConfigEntryRecord {
                    emitted_json: "{}".into(),
                },
            );
        lock
    }

    #[test]
    fn a_shared_item_keeps_its_project_outputs_in_the_committed_lock() {
        let (committed, local) = split(shared(), &project(&[".claude"]));
        let outputs = |lock: &LockFile| {
            lock.items["skill/plan"]
                .outputs
                .iter()
                .map(|output| output.target_root.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(outputs(&committed), [".mars", ".claude"]);
        assert_eq!(outputs(&local), [".agy"]);
        assert!(committed.config_entries.is_empty());
        assert!(local.config_entries.contains_key(".agy"));
    }

    #[test]
    fn a_target_the_project_names_leaves_nothing_in_the_machine_file() {
        let (committed, local) = split(shared(), &project(&[".claude", ".agy"]));
        assert!(local.is_empty());
        assert_eq!(committed.items["skill/plan"].outputs.len(), 3);
    }

    #[test]
    fn self_is_a_dependency_only_where_an_item_uses_it() {
        let mut lock = LockFile::empty();
        lock.items.insert(
            "hook/x@agy".into(),
            item("_self", ItemKind::Hook, &[(".agy", "hooks/x", "sha256:h")]),
        );
        record_local_dependency(&lock.items, &mut lock.dependencies);
        let (committed, local) = split(lock, &project(&[".claude"]));
        assert!(committed.dependencies.is_empty());
        assert!(committed.items.is_empty());
        assert_eq!(local.dependencies.len(), 1);
    }

    #[test]
    fn join_inverts_split_and_the_machine_output_wins_a_duplicate() {
        let whole = shared();
        let (committed, local) = split(whole.clone(), &project(&[".claude"]));
        let mut joined = committed.clone();
        joined.join(local.clone());
        assert_eq!(joined.items["skill/plan"].outputs.len(), 3);
        assert_eq!(joined.config_entries, whole.config_entries);

        // The same `.agy` output in both files, as after a crash mid-move: one copy, the
        // machine's.
        let mut duplicate = committed;
        duplicate
            .items
            .get_mut("skill/plan")
            .unwrap()
            .outputs
            .push(OutputRecord::installed(
                ".agy".into(),
                "skills/plan".into(),
                "sha256:stale".into(),
            ));
        duplicate.join(local);
        let agy: Vec<_> = duplicate.items["skill/plan"]
            .outputs
            .iter()
            .filter(|output| output.target_root == ".agy")
            .collect();
        assert_eq!(agy.len(), 1);
        assert_eq!(agy[0].installed_checksum().unwrap().as_ref(), "sha256:a");
    }

    #[test]
    fn the_canonical_store_never_leaves_the_committed_lock() {
        let (committed, _) = split(shared(), &project(&[]));
        assert!(
            committed.items["skill/plan"]
                .outputs
                .iter()
                .any(|output| output.target_root == CANONICAL_TARGET_ROOT)
        );
    }
}
