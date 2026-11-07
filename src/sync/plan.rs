use crate::diagnostic::DiagnosticCollector;
use crate::lock::{ItemId, LockedItem};
use crate::sync::diff::{DiffEntry, SyncDiff};
use crate::sync::target::TargetItem;
use crate::sync::types::SyncOptions;
use crate::types::{ContentHash, DestPath, SourceName};

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub actions: Vec<PlannedAction>,
}

/// A single planned action derived from a diff entry.
///
/// The plan accounts for `--force` (all conflicts become `Overwrite`)
/// and `--diff` (plan is computed but not executed).
#[derive(Debug, Clone)]
pub enum PlannedAction {
    /// Copy source content to destination.
    Install { target: TargetItem },
    /// Overwrite existing file with new source content.
    Overwrite { target: TargetItem },
    /// Skip — no changes needed.
    Skip {
        item_id: ItemId,
        dest_path: DestPath,
        source_name: SourceName,
        installed_checksum: Option<ContentHash>,
    },
    /// Remove an orphaned item.
    Remove { locked: LockedItem },
    /// Keep the local modification.
    KeepLocal {
        item_id: ItemId,
        dest_path: DestPath,
        source_name: SourceName,
    },
}

pub fn create(diff: &SyncDiff, options: &SyncOptions, diag: &mut DiagnosticCollector) -> SyncPlan {
    let mut actions = Vec::new();

    for entry in &diff.items {
        match entry {
            DiffEntry::Add { target } => {
                actions.push(PlannedAction::Install {
                    target: target.clone(),
                });
            }

            DiffEntry::Update { target } => {
                actions.push(PlannedAction::Overwrite {
                    target: target.clone(),
                });
            }

            DiffEntry::Unchanged { target, locked } => {
                actions.push(PlannedAction::Skip {
                    item_id: target.id.clone(),
                    dest_path: target.dest_path.clone(),
                    source_name: target.source_name.clone(),
                    installed_checksum: Some(locked.installed_checksum.clone()),
                });
            }

            DiffEntry::Conflict { target } => {
                if !options.force {
                    diag.warn(
                        "conflict-overwrite",
                        format!(
                            "{} `{}` has local modifications — overwriting with upstream",
                            target.id.kind, target.id.name
                        ),
                    );
                }

                actions.push(PlannedAction::Overwrite {
                    target: target.clone(),
                });
            }

            DiffEntry::Orphan { locked } => {
                actions.push(PlannedAction::Remove {
                    locked: locked.clone(),
                });
            }

            DiffEntry::LocalModified { target } => {
                if options.force {
                    actions.push(PlannedAction::Overwrite {
                        target: target.clone(),
                    });
                } else {
                    actions.push(PlannedAction::KeepLocal {
                        item_id: target.id.clone(),
                        dest_path: target.dest_path.clone(),
                        source_name: target.source_name.clone(),
                    });
                }
            }
        }
    }

    SyncPlan { actions }
}
