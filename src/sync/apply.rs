use std::path::Path;

use crate::error::MarsError;
use crate::fs::{atomic_install_dir, atomic_write};
use crate::lock::ItemId;
use crate::platform::fs as fs_ops;
use crate::sync::plan::{PlannedAction, SyncPlan};
use crate::sync::target::TargetItem;
pub use crate::sync::types::SyncOptions;
use crate::types::{ContentHash, DestPath, SourceName};
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

/// The result of applying the sync plan.
#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub outcomes: Vec<ActionOutcome>,
}

/// What action was taken for a single item.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub item_id: ItemId,
    pub action: ActionTaken,
    pub dest_path: DestPath,
    /// Which source this item came from.
    pub source_name: SourceName,
    /// Source checksum (pre-rewrite hash of source content).
    pub source_checksum: Option<ContentHash>,
    /// Installed checksum (post-rewrite hash of what was written to disk).
    pub installed_checksum: Option<ContentHash>,
}

/// The specific action taken.
#[derive(Debug, Clone)]
pub enum ActionTaken {
    Installed,
    Updated,
    Removed,
    Skipped,
    Kept,
}

/// Execute the sync plan, applying changes to disk.
///
/// For each action:
/// - Install: copy source content to dest (atomic_write or atomic_install_dir)
/// - Overwrite: replace existing with new source content
/// - Remove: delete file/dir from disk
/// - Skip/KeepLocal: record as no-op
///
/// Returns outcomes with both source_checksum and installed_checksum.
/// The installed_checksum may differ from source_checksum when frontmatter
/// rewriting occurred.
pub fn execute(
    root: &Path,
    plan: &SyncPlan,
    options: &SyncOptions,
) -> Result<ApplyResult, MarsError> {
    let mut outcomes = Vec::new();

    for action in &plan.actions {
        let outcome = if options.dry_run {
            // Dry run: compute the outcome without touching disk
            dry_run_action(action)
        } else {
            execute_action(root, action)?
        };
        outcomes.push(outcome);
    }

    Ok(ApplyResult { outcomes })
}

/// Execute a single action, writing to disk.
fn execute_action(root: &Path, action: &PlannedAction) -> Result<ActionOutcome, MarsError> {
    match action {
        PlannedAction::Install { target } => {
            let dest = target.dest_path.resolve(root);

            // Read source content and install
            let installed_checksum = install_item(target, &dest)?;

            Ok(ActionOutcome {
                item_id: target.id.clone(),
                action: ActionTaken::Installed,
                dest_path: target.dest_path.clone(),
                source_name: target.source_name.clone(),
                source_checksum: Some(target.source_hash.clone()),
                installed_checksum: Some(installed_checksum),
            })
        }

        PlannedAction::Overwrite { target } => {
            let dest = target.dest_path.resolve(root);

            // Install (overwrite) source content
            let installed_checksum = install_item(target, &dest)?;

            Ok(ActionOutcome {
                item_id: target.id.clone(),
                action: ActionTaken::Updated,
                dest_path: target.dest_path.clone(),
                source_name: target.source_name.clone(),
                source_checksum: Some(target.source_hash.clone()),
                installed_checksum: Some(installed_checksum),
            })
        }

        PlannedAction::Remove { locked } => {
            let dest = removal_path(root, &locked.dest_path, locked.kind);
            if dest.exists() {
                fs_ops::safe_remove(&dest)?;
            }

            let item_id = ItemId {
                kind: locked.kind,
                name: ItemName::from(locked.dest_path.item_name(locked.kind)),
            };

            Ok(ActionOutcome {
                item_id,
                action: ActionTaken::Removed,
                dest_path: locked.dest_path.clone(),
                source_name: locked.source.clone(),
                source_checksum: None,
                installed_checksum: None,
            })
        }

        PlannedAction::Skip {
            item_id,
            dest_path,
            source_name,
            installed_checksum,
        } => Ok(ActionOutcome {
            item_id: item_id.clone(),
            action: ActionTaken::Skipped,
            dest_path: dest_path.clone(),
            source_name: source_name.clone(),
            source_checksum: None,
            installed_checksum: installed_checksum.clone(),
        }),

        PlannedAction::KeepLocal {
            item_id,
            dest_path,
            source_name,
        } => Ok(ActionOutcome {
            item_id: item_id.clone(),
            action: ActionTaken::Kept,
            dest_path: dest_path.clone(),
            source_name: source_name.clone(),
            source_checksum: None,
            installed_checksum: None,
        }),
    }
}

/// Produce a dry-run outcome without touching disk.
fn dry_run_action(action: &PlannedAction) -> ActionOutcome {
    match action {
        PlannedAction::Install { target } => ActionOutcome {
            item_id: target.id.clone(),
            action: ActionTaken::Installed,
            dest_path: target.dest_path.clone(),
            source_name: target.source_name.clone(),
            source_checksum: Some(target.source_hash.clone()),
            installed_checksum: None, // Can't know without actually installing
        },
        PlannedAction::Overwrite { target } => ActionOutcome {
            item_id: target.id.clone(),
            action: ActionTaken::Updated,
            dest_path: target.dest_path.clone(),
            source_name: target.source_name.clone(),
            source_checksum: Some(target.source_hash.clone()),
            installed_checksum: None,
        },
        PlannedAction::Remove { locked } => {
            let item_id = ItemId {
                kind: locked.kind,
                name: ItemName::from(locked.dest_path.item_name(locked.kind)),
            };
            ActionOutcome {
                item_id,
                action: ActionTaken::Removed,
                dest_path: locked.dest_path.clone(),
                source_name: locked.source.clone(),
                source_checksum: None,
                installed_checksum: None,
            }
        }
        PlannedAction::Skip {
            item_id,
            dest_path,
            source_name,
            installed_checksum,
            ..
        } => ActionOutcome {
            item_id: item_id.clone(),
            action: ActionTaken::Skipped,
            dest_path: dest_path.clone(),
            source_name: source_name.clone(),
            source_checksum: None,
            installed_checksum: installed_checksum.clone(),
        },
        PlannedAction::KeepLocal {
            item_id,
            dest_path,
            source_name,
        } => ActionOutcome {
            item_id: item_id.clone(),
            action: ActionTaken::Kept,
            dest_path: dest_path.clone(),
            source_name: source_name.clone(),
            source_checksum: None,
            installed_checksum: None,
        },
    }
}

/// Install an item (file or directory) to the destination.
///
/// Returns the installed checksum (hash of what was written to disk).
fn install_item(target: &TargetItem, dest: &Path) -> Result<ContentHash, MarsError> {
    match target.id.kind {
        ItemKind::Agent => {
            let content = content_to_install(target)?;
            write_file_and_verify(dest, &content)
        }
        ItemKind::BootstrapDoc => {
            let doc_dest = dest.parent().ok_or_else(|| {
                std::io::Error::other(format!(
                    "bootstrap destination has no parent directory: {}",
                    dest.display()
                ))
            })?;
            atomic_install_dir(&target.source_path, doc_dest)?;
            crate::hash::compute_hash(doc_dest, ItemKind::BootstrapDoc).map(ContentHash::from)
        }
        ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => {
            if target.is_flat_skill {
                crate::fs::atomic_install_dir_filtered(
                    &target.source_path,
                    dest,
                    crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
                )?;
            } else {
                atomic_install_dir(&target.source_path, dest)?;
            }
            if target.id.kind == ItemKind::Skill
                && let Some(content) = &target.rewritten_content
            {
                crate::fs::atomic_write_preserving_permissions(
                    &dest.join("SKILL.md"),
                    content.as_bytes(),
                )?;
            }
            // Directory items are verified by hashing the installed content.
            crate::hash::compute_hash(dest, target.id.kind).map(ContentHash::from)
        }
    }
}

/// Write bytes to `dest` and verify persisted bytes hash matches expected.
fn write_file_and_verify(dest: &Path, content: &[u8]) -> Result<ContentHash, MarsError> {
    atomic_write(dest, content)?;
    let expected = ContentHash::from(crate::hash::hash_bytes(content));
    let persisted = std::fs::read(dest)?;
    let actual = ContentHash::from(crate::hash::hash_bytes(&persisted));
    if expected != actual {
        return Err(std::io::Error::other(format!(
            "post-write verification failed for {}: expected {expected}, got {actual}",
            dest.display()
        ))
        .into());
    }
    Ok(actual)
}

/// Read bytes to install for an agent, honoring in-memory rewrite overrides.
fn content_to_install(target: &TargetItem) -> Result<Vec<u8>, MarsError> {
    if let Some(content) = &target.rewritten_content {
        Ok(content.as_bytes().to_vec())
    } else if target.id.kind == ItemKind::BootstrapDoc {
        Ok(std::fs::read(target.source_path.join("BOOTSTRAP.md"))?)
    } else {
        Ok(std::fs::read(&target.source_path)?)
    }
}

fn removal_path(root: &Path, dest_path: &DestPath, kind: ItemKind) -> std::path::PathBuf {
    let dest = dest_path.resolve(root);
    if kind == ItemKind::BootstrapDoc {
        if dest_path.as_str().split('/').count() >= 3 {
            dest.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dest.clone())
        } else {
            dest
        }
    } else {
        dest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::LockedItem;
    use crate::sync::plan::{PlannedAction, SyncPlan};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn remove_degenerate_bootstrap_doc_path_removes_exact_file_only() {
        let root = TempDir::new().unwrap();
        let bootstrap_dir = root.path().join("bootstrap");
        fs::create_dir_all(&bootstrap_dir).unwrap();
        fs::write(bootstrap_dir.join("BOOTSTRAP.md"), b"# root").unwrap();
        fs::write(bootstrap_dir.join("keep.md"), b"# keep").unwrap();

        let locked = LockedItem {
            source: "old-source".into(),
            kind: ItemKind::BootstrapDoc,
            version: None,
            source_checksum: "sha256:aaa".into(),
            installed_checksum: "sha256:bbb".into(),
            dest_path: "bootstrap/BOOTSTRAP.md".into(),
        };

        let plan = SyncPlan {
            actions: vec![PlannedAction::Remove { locked }],
        };
        let options = SyncOptions::default();

        let result = execute(root.path(), &plan, &options).unwrap();
        assert!(matches!(result.outcomes[0].action, ActionTaken::Removed));
        assert!(!bootstrap_dir.join("BOOTSTRAP.md").exists());
        assert!(bootstrap_dir.join("keep.md").exists());
    }
}
