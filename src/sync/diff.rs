use std::path::Path;

use crate::error::MarsError;
use crate::hash;
use crate::lock::{CANONICAL_TARGET_ROOT, LockFile, LockIndex, LockedItem};
use crate::sync::target::{TargetItem, TargetState};
use crate::types::ContentHash;

#[derive(Debug, Clone)]
pub struct SyncDiff {
    pub items: Vec<DiffEntry>,
}

#[derive(Debug, Clone)]
pub enum DiffEntry {
    /// New item not in lock or on disk.
    Add { target: TargetItem },
    /// Source changed, local unchanged → clean update.
    Update { target: TargetItem },
    /// Source unchanged, local unchanged → skip.
    Unchanged {
        target: TargetItem,
        locked: LockedItem,
    },
    /// Source changed AND local changed → needs merge.
    Conflict { target: TargetItem },
    /// In lock but not in target → should be removed.
    Orphan { locked: LockedItem },
    /// Local modification, source unchanged → keep local.
    LocalModified { target: TargetItem },
}

pub fn compute(
    root: &Path,
    lock: &LockFile,
    target: &TargetState,
    force: bool,
) -> Result<SyncDiff, MarsError> {
    let mut items = Vec::new();
    let lock_index = LockIndex::new(lock);

    for (_dest_key, target_item) in &target.items {
        if let Some(locked_item) =
            lock_index.find_output(CANONICAL_TARGET_ROOT, &target_item.dest_path)
        {
            let effective_installed = target_item.expected_installed_hash()?;
            let source_changed = target_item.source_hash != locked_item.source_checksum
                || effective_installed != locked_item.installed_checksum;

            // In --force mode, baseline is source_checksum so conflicted files
            // are treated as local modifications and get overwritten.
            let expected_disk_checksum = if force {
                &locked_item.source_checksum
            } else {
                &locked_item.installed_checksum
            };

            let disk_path = target_item.dest_path.resolve(root);
            let local_changed = if disk_path.exists() {
                let disk_hash = hash::compute_hash(&disk_path, target_item.id.kind)?;
                let disk_hash = ContentHash::from(disk_hash);
                if disk_hash != *expected_disk_checksum {
                    Some(disk_hash)
                } else {
                    None
                }
            } else {
                None
            };

            match (source_changed, &local_changed) {
                (false, None) => {
                    if disk_path.exists() && target_item.source_name != locked_item.source {
                        // Identical bytes can still transfer ownership. A write outcome
                        // records the selected source and carries native claims forward.
                        // Locally modified outputs still take LocalModified below.
                        items.push(DiffEntry::Update {
                            target: target_item.clone(),
                        });
                    } else if disk_path.exists() {
                        items.push(DiffEntry::Unchanged {
                            target: target_item.clone(),
                            locked: locked_item.clone(),
                        });
                    } else {
                        items.push(DiffEntry::Add {
                            target: target_item.clone(),
                        });
                    }
                }
                (true, None) => {
                    items.push(DiffEntry::Update {
                        target: target_item.clone(),
                    });
                }
                (false, Some(_local_hash)) => {
                    items.push(DiffEntry::LocalModified {
                        target: target_item.clone(),
                    });
                }
                (true, Some(_local_hash)) => {
                    items.push(DiffEntry::Conflict {
                        target: target_item.clone(),
                    });
                }
            }
        } else {
            items.push(DiffEntry::Add {
                target: target_item.clone(),
            });
        }
    }

    // A kept hook's directories stay as the lock has them, whatever the disk or the
    // (invalid) source holds.
    let mut kept = std::collections::HashSet::new();
    for dir in target.kept_dirs() {
        let dest = dir.canonical();
        if let Some(locked) = lock_index.find_output(CANONICAL_TARGET_ROOT, &dest) {
            items.push(DiffEntry::Unchanged {
                target: crate::sync::target::package_dir_target_item(
                    dir,
                    Path::new(""),
                    &locked.source,
                    &locked.source_checksum,
                ),
                locked,
            });
            kept.insert(dest);
        }
    }

    for (dest_path, locked_item) in lock.canonical_flat_items() {
        if !target.items.contains_key(&dest_path) && !kept.contains(&dest_path) {
            items.push(DiffEntry::Orphan {
                locked: locked_item,
            });
        }
    }

    Ok(SyncDiff { items })
}
