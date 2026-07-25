//! Per-target surface ownership — gate linked-target mutations on lock records.
//!
//! For linked targets (`.cursor`, `.claude`, etc.), Mars may delete or overwrite
//! only when the lock has an [`OutputRecord`](crate::lock::OutputRecord) for
//! `(target_root, dest_path)`. `.mars`-only records do not imply ownership
//! elsewhere.
//!
//! Path ownership has two explicit lifecycle claims. Deletion authority and
//! replacement authority are distinct. Either lifecycle state authorizes deletion
//! of the recorded path. Overwriting an existing path without `--force` requires
//! an installed record, whose checksum describes content at the path. A
//! pending-deletion record carries removal-retry authority only: it has no
//! checksum and must be treated as a copy/install collision. `--force` may
//! explicitly adopt that path and restore its state to installed.
//!
//! Merge-mode config entries use entry ownership rather than path ownership. A
//! config entry may be removed only when the lock holds a
//! [`ConfigEntryRecord`](crate::lock::ConfigEntryRecord) and its recorded
//! `emitted_json` still matches the entry on disk. The target adapters enforce
//! that structural match in `remove_owned_*_hooks`; content resemblance without
//! a lock record never establishes ownership. The temporary issue #130
//! old-lock bridge is narrower: a record without `emitted_json` authorizes only
//! the legacy command-path cleanup associated with that recorded key.
//!
//! [`retention`] enforces the other half of the contract: each removal is a
//! single-use operation that reports the records it could not confirm removing.
//! A confirmed pair can bind its target, surface, and payload into one write
//! operation; an unconfirmed pair cannot. This preserves retry authority instead
//! of silently replacing ownership evidence after a failed sweep.

pub(crate) mod retention;

use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::lock::LockFile;
use crate::types::managed_cmd;

/// Which command's `--force` flag would adopt an untracked collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionAdoptHint {
    SyncForce,
    LinkForce,
}

/// Whether a copy/install to a linked target may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCopyDecision {
    /// Dest is missing, has an installed-content claim, or `--force` adopt applies.
    Proceed,
    /// Dest exists without an installed-content claim — preserve local content.
    SkipWithoutInstalledClaim,
}

/// Whether `dest` exists on disk (file, directory, or symlink).
pub fn target_dest_exists(dest: &Path) -> bool {
    dest.exists() || dest.symlink_metadata().is_ok()
}

/// Whether Mars may delete `dest_path` under `target_root`.
pub fn may_delete(old_lock: &LockFile, target_root: &str, dest_path: &str) -> bool {
    old_lock.contains_output(target_root, dest_path)
}

/// Decide whether Mars may copy/install to a linked target path.
pub fn copy_decision(
    old_lock: &LockFile,
    target_root: &str,
    dest_path: &str,
    dest_exists: bool,
    force: bool,
) -> SurfaceCopyDecision {
    if !dest_exists {
        return SurfaceCopyDecision::Proceed;
    }
    if old_lock
        .installed_checksum_for_output(target_root, dest_path)
        .is_some()
    {
        return SurfaceCopyDecision::Proceed;
    }
    if force {
        return SurfaceCopyDecision::Proceed;
    }
    SurfaceCopyDecision::SkipWithoutInstalledClaim
}

/// Emit `target-unmanaged-collision` when an existing path lacks an installed claim.
pub fn warn_no_installed_claim_collision(
    target_name: &str,
    dest_rel: &str,
    hint: CollisionAdoptHint,
    diag: &mut DiagnosticCollector,
) {
    let adopt_cmd: String = match hint {
        CollisionAdoptHint::SyncForce => managed_cmd("mars sync --force").into_owned(),
        CollisionAdoptHint::LinkForce => {
            let inner = format!("mars link {target_name} --force");
            managed_cmd(&inner).into_owned()
        }
    };
    diag.warn(
        "target-unmanaged-collision",
        format!(
            "target `{target_name}` item `{dest_rel}` exists locally but has no installed-content claim \
             (preserved local content; run `{adopt_cmd}` to adopt)"
        ),
    );
}

/// Emit `target-unmanaged-adopted` when `--force` adopts a path without an installed claim.
pub fn warn_no_installed_claim_adopted(
    target_name: &str,
    dest_rel: &str,
    hint: CollisionAdoptHint,
    diag: &mut DiagnosticCollector,
) {
    let _ = hint;
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
    use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
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
                    outputs: vec![OutputRecord::installed(
                        target_root.to_string(),
                        dest_path.into(),
                        "sha256:inst".into(),
                    )],
                },
            )]),
            config_entries: Default::default(),
            dependency_model_aliases: IndexMap::new(),
        }
    }

    #[test]
    fn copy_decision_proceeds_when_dest_missing() {
        let lock = LockFile::empty();
        assert_eq!(
            copy_decision(&lock, ".cursor", "agents/coder.md", false, false),
            SurfaceCopyDecision::Proceed
        );
    }

    #[test]
    fn copy_decision_skips_untracked_collision_without_force() {
        let lock = lock_with_output(".mars", "agents/coder.md");
        assert_eq!(
            copy_decision(&lock, ".cursor", "agents/coder.md", true, false),
            SurfaceCopyDecision::SkipWithoutInstalledClaim
        );
    }

    #[test]
    fn copy_decision_proceeds_when_target_tracked() {
        let lock = lock_with_output(".cursor", "agents/coder.md");
        assert_eq!(
            copy_decision(&lock, ".cursor", "agents/coder.md", true, false),
            SurfaceCopyDecision::Proceed
        );
    }

    #[test]
    fn copy_decision_treats_pending_deletion_as_collision() {
        let mut lock = lock_with_output(".cursor", "agents/coder.md");
        lock.items.get_mut("agent/coder").unwrap().outputs[0].mark_pending_deletion();

        assert_eq!(
            copy_decision(&lock, ".cursor", "agents/coder.md", true, false),
            SurfaceCopyDecision::SkipWithoutInstalledClaim
        );
        assert_eq!(
            copy_decision(&lock, ".cursor", "agents/coder.md", true, true),
            SurfaceCopyDecision::Proceed
        );
        assert!(
            may_delete(&lock, ".cursor", "agents/coder.md"),
            "a tombstone still carries deletion authority"
        );
    }

    #[test]
    fn may_delete_requires_per_target_output_record() {
        let lock = lock_with_output(".mars", "agents/coder.md");
        assert!(!may_delete(&lock, ".cursor", "agents/coder.md"));
        assert!(may_delete(&lock, ".mars", "agents/coder.md"));
    }

    #[test]
    fn collision_diagnostic_describes_missing_installed_content_claim() {
        let mut diagnostics = DiagnosticCollector::new();

        warn_no_installed_claim_collision(
            ".cursor",
            "agents/coder.md",
            CollisionAdoptHint::SyncForce,
            &mut diagnostics,
        );

        let diagnostics = diagnostics.drain();
        assert_eq!(diagnostics.len(), 1);
        let message = &diagnostics[0].message;
        assert!(message.contains("has no installed-content claim"));
        assert!(!message.contains("not tracked by Mars"));
    }
}
