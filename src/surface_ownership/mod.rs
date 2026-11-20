//! Per-target surface ownership — gate linked-target mutations on lock records.
//!
//! For linked targets (`.cursor`, `.claude`, etc.), Mars may delete only when
//! the lock has an [`OutputRecord`](crate::lock::OutputRecord) for
//! `(target_root, dest_path)`, and may overwrite without `--force` only when
//! that record asserts installed content. `.mars`-only records do not imply
//! ownership elsewhere.
//!
//! Path ownership has two explicit lifecycle claims. Deletion authority and
//! replacement authority are distinct. Either lifecycle state authorizes deletion
//! of the recorded path. Overwriting an existing path without `--force` requires
//! an installed record, whose checksum describes content at the path. A
//! pending-deletion record carries removal-retry authority only: it has no
//! checksum and must be treated as a copy/install collision. `--force` may
//! explicitly adopt that path and restore its state to installed.
//!
//! Config entries use entry ownership rather than path ownership: an entry may
//! be removed only when the lock holds a
//! [`ConfigEntryRecord`](crate::lock::ConfigEntryRecord) for its key. Content
//! resemblance without a lock record never establishes ownership.
//!
//! Merge-mode hooks are also matched by content: a hook entry is removed only
//! when its recorded `emitted_json` still matches the entry on disk.
//!
//! MCP servers are matched by content too: a record's `emitted_json` is the
//! entry mars wrote, so an entry on disk that still equals it is mars's to
//! replace or remove, and one that differs was edited and is preserved
//! (`target::keyed_entries::reconcile`, which owns MCP removal and write in one pass).
//!
//! [`retention`] enforces the other half of the contract for hooks, which are
//! removed whole before they are rewritten: each removal is a single-use
//! operation that reports the records it could not confirm removing. A
//! confirmed target can bind its payload into one write operation; an
//! unconfirmed target cannot. This preserves retry authority instead of
//! silently replacing ownership evidence after a failed sweep.

#[cfg(test)]
use mars_schema::item::ItemKind;
pub(crate) mod retention;

use std::path::Path;

use crate::diagnostic::{DiagnosticCollector, ItemGroup};
use crate::lock::LockFile;
use crate::types::managed_cmd;

pub fn target_dest_exists(dest: &Path) -> bool {
    dest.exists() || dest.symlink_metadata().is_ok()
}

/// What sits at a path Mars is about to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestState {
    Absent,
    /// Something Mars did not just write there: an installed claim says whether it is Mars's.
    Present,
    /// Exactly the content Mars would write.
    Identical,
}

impl DestState {
    pub fn new(exists: bool, identical: bool) -> Self {
        match (exists, identical) {
            (false, _) => Self::Absent,
            (true, false) => Self::Present,
            (true, true) => Self::Identical,
        }
    }
}

/// Whether Mars may write `dest_path`, saying why not when it may not.
///
/// A path with an installed claim is Mars's to write. One without a claim is the user's, unless
/// `--force` adopts it or its content is already what Mars would write (`Identical`): a sync
/// interrupted before its lock write leaves exactly that, and adopting it needs no user say.
pub fn resolve_copy(
    old_lock: &LockFile,
    target_root: &str,
    dest_path: &str,
    dest: DestState,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> bool {
    if dest != DestState::Present
        || old_lock
            .installed_checksum_for_output(target_root, dest_path)
            .is_some()
    {
        return true;
    }
    if force {
        warn_no_installed_claim_adopted(target_root, dest_path, diag);
        return true;
    }
    warn_no_installed_claim_collision(target_root, dest_path, diag);
    false
}

pub fn warn_no_installed_claim_collision(
    target_name: &str,
    dest_rel: &str,
    diag: &mut DiagnosticCollector,
) {
    diag.warn_each(
        ItemGroup {
            code: "target-unmanaged-collision",
            subject: target_name,
            describe: describe_unclaimed_by_sync,
        },
        format!("`{dest_rel}`"),
    );
}

fn describe_unclaimed(target: &str, count: usize, names: &str, adopt_cmd: &str) -> String {
    let plural = if count == 1 { "item" } else { "items" };
    format!(
        "target `{target}` has {count} {plural} that exist locally with no installed-content \
         claim: {names}. They are preserved; if they are files Mars wrote and lost track of, or \
         ones you want replaced, run `{adopt_cmd}` to adopt them"
    )
}

fn describe_unclaimed_by_sync(target: &str, count: usize, names: &str) -> String {
    describe_unclaimed(target, count, names, &managed_cmd("mars sync --force"))
}

/// Whether `warn_no_installed_claim_collision` or `warn_diverged` already told the user that
/// `dest_rel` of `target_name` was left alone, so what follows from that (a config entry that
/// cannot name the missing copy) need not say it again.
pub fn already_left_alone(target_name: &str, dest_rel: &str, diag: &DiagnosticCollector) -> bool {
    let item = format!("`{dest_rel}`");
    ["target-unmanaged-collision", "target-divergent"]
        .iter()
        .any(|code| diag.has_item(code, target_name, &item))
}

/// An item of `target` whose installed content was edited after Mars wrote it: it is
/// preserved, and `--force` or `repair` resets it. One line per target.
pub fn warn_diverged(target_name: &str, dest_rel: &str, diag: &mut DiagnosticCollector) {
    diag.warn_each(
        ItemGroup {
            code: "target-divergent",
            subject: target_name,
            describe: describe_diverged,
        },
        format!("`{dest_rel}`"),
    );
}

fn describe_diverged(target: &str, count: usize, names: &str) -> String {
    let plural = if count == 1 { "item" } else { "items" };
    format!(
        "target `{target}` has {count} {plural} edited after Mars installed {them}: {names}. \
         Local content is preserved; run `{}` or `{}` to reset",
        managed_cmd("mars sync --force"),
        managed_cmd("mars repair"),
        them = if count == 1 { "it" } else { "them" },
    )
}

pub fn warn_no_installed_claim_adopted(
    target_name: &str,
    dest_rel: &str,
    diag: &mut DiagnosticCollector,
) {
    diag.warn(
        "target-unmanaged-adopted",
        format!(
            "target `{target_name}` item `{dest_rel}` existed but had no installed-content claim; \
             adopting with `--force`"
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{LockFile, LockedItemV2, OutputRecord};
    use indexmap::IndexMap;

    fn lock_with_output(target_root: &str, dest_path: &str) -> LockFile {
        LockFile {
            version: 3,
            dependencies: IndexMap::new(),
            items: IndexMap::from([(
                "agent/coder".to_string(),
                LockedItemV2 {
                    source: "test".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:src".into(),
                    source_path: None,
                    outputs: vec![OutputRecord::installed(
                        target_root.to_string(),
                        dest_path.into(),
                        "sha256:inst".into(),
                    )],
                },
            )]),
            config_entries: Default::default(),
            provided_entries: Default::default(),
            dependency_model_aliases: IndexMap::new(),
        }
    }

    fn resolve(lock: &LockFile, identical: bool, force: bool) -> (bool, DiagnosticCollector) {
        let mut diag = DiagnosticCollector::new();
        let copy = resolve_copy(
            lock,
            ".cursor",
            "agents/coder.md",
            DestState::new(true, identical),
            force,
            &mut diag,
        );
        (copy, diag)
    }

    #[test]
    fn resolve_copy_skips_untracked_collision_without_force() {
        let lock = lock_with_output(".mars", "agents/coder.md");
        assert!(!resolve(&lock, false, false).0);
    }

    #[test]
    fn resolve_copy_adopts_identical_content_without_a_word() {
        let lock = lock_with_output(".mars", "agents/coder.md");
        let (copy, mut diag) = resolve(&lock, true, false);
        assert!(copy);
        assert!(diag.drain().is_empty());
    }

    #[test]
    fn resolve_copy_treats_pending_deletion_as_collision() {
        let mut lock = lock_with_output(".cursor", "agents/coder.md");
        lock.items.get_mut("agent/coder").unwrap().outputs[0].mark_pending_deletion();

        assert!(!resolve(&lock, false, false).0);
        assert!(resolve(&lock, false, true).0);
    }
}
