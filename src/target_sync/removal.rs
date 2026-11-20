//! Remove everything Mars installed for a set of targets: `mars unlink` and a sync that finds a
//! target no config file names both end here, so what leaves is the same.

use std::path::Path;

use crate::diagnostic::{Diagnostic, DiagnosticCollector};
use crate::error::MarsError;
use crate::lock::LockFile;

/// The `(target root, dest path)` of each output a lock holds, deepest first so a directory's
/// contents go before it. Includes the `.mars/<surface>/<target>/` copies of per-target items.
pub(crate) fn served_outputs(served: &LockFile) -> Vec<(String, String)> {
    let mut outputs: Vec<(String, String)> = served
        .items
        .values()
        .flat_map(|item| item.outputs.iter())
        .map(|output| (output.target_root.clone(), output.dest_path.to_string()))
        .collect();
    outputs.sort_by_key(|(_, path)| std::cmp::Reverse(path.matches('/').count()));
    outputs
}

/// Delete what `served` records (files, hook and MCP entries, the empty directories they leave)
/// and put back into `lock` any record whose removal could not be confirmed, such as an entry
/// the user edited. `served` is the half of the lock `LockFile::split_targets` set aside; a
/// crash before the caller writes `lock` leaves the records, and the next run removes the rest.
pub(crate) fn remove_served(
    project_root: &Path,
    lock: &mut LockFile,
    served: LockFile,
) -> Result<Vec<Diagnostic>, MarsError> {
    for (root, dest_path) in served_outputs(&served) {
        let root_dir = project_root.join(&root);
        let path = root_dir.join(&dest_path);
        match remove_owned_path(&path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    remove_empty_ancestors(parent, &root_dir)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let targets = served.recorded_targets();
    let mut diagnostics = DiagnosticCollector::new();
    let retention = crate::surface_ownership::retention::RemovalPlan::build(
        &served.config_entries,
        &std::collections::BTreeMap::new(),
    )
    .execute(
        |operation, diagnostics| {
            crate::compiler::config_entries::remove_hooks(operation, project_root, diagnostics)
        },
        &mut diagnostics,
    );
    let reconciled = crate::compiler::config_entries::reconcile_all_keyed(
        project_root,
        &served.config_entries,
        &std::collections::BTreeMap::new(),
        false,
        false,
        &mut diagnostics,
    );
    crate::compiler::config_entries::report_removed(
        retention.removed_keys(),
        &reconciled.removed,
        &mut diagnostics,
    );
    lock.config_entries
        .extend(retention.into_retained_records());
    for (target, records) in reconciled.records {
        lock.config_entries
            .entry(target)
            .or_default()
            .extend(records);
    }
    // A record put back stays in the file it came from.
    let restored: Vec<_> = served
        .provided_entries
        .into_iter()
        .filter(|(target, key)| {
            lock.config_entries
                .get(target)
                .is_some_and(|records| records.contains_key(key))
        })
        .collect();
    lock.provided_entries.extend(restored);

    // Removing config entries can empty a directory the outputs loop already passed.
    for target in targets {
        let dir = project_root.join(&target);
        remove_empty_ancestors(&dir, &dir)?;
    }
    Ok(diagnostics.drain())
}

fn remove_owned_path(path: &Path) -> std::io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
}

/// Remove `start` and each parent up to and including `target_dir` while they are empty.
/// Whether `target_dir` itself went.
pub(crate) fn remove_empty_ancestors(start: &Path, target_dir: &Path) -> std::io::Result<bool> {
    let mut current = Some(start);
    let mut removed_target = false;
    while let Some(path) = current {
        if !path.starts_with(target_dir) {
            break;
        }
        match std::fs::remove_dir(path) {
            Ok(()) => {
                removed_target |= path == target_dir;
                current = path.parent();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = path.parent();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotADirectory
                ) =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(removed_target)
}
