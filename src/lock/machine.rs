//! Which records of a lock live in the machine-local `.mars/machine.lock` and which in the
//! committed `mars.lock`, and how the two are joined back.
//!
//! A record is machine-local when it describes one machine: content from a layer other than the
//! project's (the user level's `_user-*`, a host's `_provided-*`; `crate::layer`), or anything that serves a target `mars.toml` does not name, because only this machine's
//! `mars.local.toml` asked for it. Everything else is shared. [`split`] and [`join`] are
//! inverses over the lock a sync sees.

use std::collections::BTreeSet;

use super::targets::Away;
use super::{LockFile, record_local_dependency};
use crate::layer::is_machine_local;
use crate::types::SourceOrigin;

/// `(committed, machine-local)`. `project_targets` are the targets `mars.toml` names.
pub(crate) fn split(lock: LockFile, project_targets: &BTreeSet<String>) -> (LockFile, LockFile) {
    let provided = lock.provided_entries.clone();
    let off_project = |target: &str| !project_targets.contains(target);
    let (mut committed, mut local) = lock.split_by(&Away {
        target: &off_project,
        source: &is_machine_local,
        entry: &|target, key| {
            off_project(target) || provided.contains(&(target.to_string(), key.to_string()))
        },
    });
    // `_self` names the project's own package in the file whose items use it.
    for lock in [&mut committed, &mut local] {
        let local_source: crate::types::SourceName = SourceOrigin::LocalPackage.to_string().into();
        lock.dependencies.shift_remove(&local_source);
        record_local_dependency(&lock.items, &mut lock.dependencies);
    }
    (committed, local)
}

impl LockFile {
    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.dependencies.is_empty() && self.config_entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{CANONICAL_TARGET_ROOT, ConfigEntryRecord, LockedItemV2, OutputRecord};
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
    fn the_canonical_store_stays_committed_while_no_target_takes_it_away() {
        let (committed, _) = split(shared(), &project(&[]));
        assert!(
            committed.items["skill/plan"]
                .outputs
                .iter()
                .any(|output| output.target_root == CANONICAL_TARGET_ROOT)
        );
    }
}
