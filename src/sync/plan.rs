use crate::diagnostic::DiagnosticCollector;
use crate::lock::{ItemId, LockedItem};
use crate::sync::diff::{DiffEntry, SyncDiff};
use crate::sync::target::TargetItem;
use crate::sync::types::SyncOptions;
use crate::types::{ContentHash, DestPath, SourceName};
#[cfg(test)]
use mars_schema::item::ItemKind;

/// A planned set of actions to execute.
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

/// Create execution plan from diff.
///
/// `--force`: all Conflict entries become Overwrite (source wins).
/// `--dry_run`: plan is computed identically but not executed (handled by apply).
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

                // Source wins: overwrite local modifications
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
                    // --force: source wins even when only local changed
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use crate::lock::ItemId;
    use crate::sync::diff::{DiffEntry, SyncDiff};
    use crate::sync::target::TargetItem;
    use std::path::PathBuf;
    fn make_target(name: &str) -> TargetItem {
        TargetItem {
            id: ItemId {
                kind: ItemKind::Agent,
                name: name.into(),
            },
            source_name: "test".into(),
            source_path: PathBuf::from(format!("/tmp/source/agents/{name}.md")),
            dest_path: format!("agents/{name}.md").into(),
            source_hash: hash::hash_bytes(b"test content").into(),
            is_flat_skill: false,
            rewritten_content: None,
        }
    }

    fn default_options() -> SyncOptions {
        SyncOptions::default()
    }

    fn force_options() -> SyncOptions {
        SyncOptions {
            force: true,
            ..SyncOptions::default()
        }
    }

    fn create_plan_with_diag(
        diff: &SyncDiff,
        options: &SyncOptions,
    ) -> (SyncPlan, DiagnosticCollector) {
        let mut diag = DiagnosticCollector::new();
        let plan = create(diff, options, &mut diag);
        (plan, diag)
    }

    #[test]
    fn conflicts_and_local_edits_follow_the_force_flag() {
        // (entry, options, expected action, expected diagnostic)
        let conflict = || DiffEntry::Conflict {
            target: make_target("item"),
        };
        let local_modified = || DiffEntry::LocalModified {
            target: make_target("item"),
        };
        let cases = [
            (
                conflict(),
                default_options(),
                "Overwrite",
                Some("conflict-overwrite"),
            ),
            (local_modified(), default_options(), "KeepLocal", None),
            (local_modified(), force_options(), "Overwrite", None),
        ];
        for (entry, options, expected, code) in cases {
            let diff = SyncDiff { items: vec![entry] };
            let (plan, mut diag) = create_plan_with_diag(&diff, &options);
            assert_eq!(plan.actions.len(), 1);
            let actual = match &plan.actions[0] {
                PlannedAction::Overwrite { .. } => "Overwrite",
                PlannedAction::KeepLocal { .. } => "KeepLocal",
                other => panic!("unexpected action {other:?}"),
            };
            assert_eq!(actual, expected);
            let codes: Vec<_> = diag.drain().into_iter().map(|d| d.code).collect();
            assert_eq!(codes, code.into_iter().collect::<Vec<_>>());
        }
    }
}
