//! Remove everything Mars installed for a set of targets: `mars unlink` and a sync that finds a
//! target no config file names both end here, so what leaves is the same.

use std::collections::BTreeSet;
use std::path::Path;

use mars_schema::item::ItemKind;

use crate::diagnostic::{Diagnostic, DiagnosticCollector, ItemGroup};
use crate::error::MarsError;
use crate::lock::{LockFile, OutputRecord};

/// What a recorded output is now, set against what Mars wrote.
enum Check {
    /// Nothing is at the path.
    Absent,
    /// It is what Mars wrote, or a pending deletion that must finish.
    Own,
    /// It is not what Mars wrote, so it stays.
    Changed,
}

/// What became of one recorded output.
pub(crate) enum Removal {
    Absent,
    Removed,
    Kept,
    Failed(MarsError),
}

/// "What Mars wrote" is the record's `installed_checksum` compared with the hash of the whole
/// output as the lock computes it: a file for an agent, the tree for a skill, hook or MCP
/// server. A file the user added inside a recorded directory changes the tree hash, and so does
/// a tree that cannot be hashed (a dangling symlink, a FIFO): Mars cannot show it is its own
/// copy, so it is `Changed`. A pending-deletion record has no checksum and is `Own`: it exists
/// to retry a removal Mars had already decided on.
fn check(path: &Path, kind: ItemKind, record: &OutputRecord) -> Result<Check, MarsError> {
    match path.symlink_metadata() {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Check::Absent),
        Err(error) => return Err(MarsError::io("read metadata for", path, error)),
    }
    let Some(recorded) = record.installed_checksum() else {
        return Ok(Check::Own);
    };
    let current = crate::hash::compute_output_hash(path, kind, record.dest_path.as_str());
    Ok(if current.ok().as_deref() == Some(recorded.as_ref()) {
        Check::Own
    } else {
        Check::Changed
    })
}

/// Remove the output `record` names at `path`, unless it is no longer what Mars wrote.
pub(crate) fn remove_recorded(path: &Path, kind: ItemKind, record: &OutputRecord) -> Removal {
    match check(path, kind, record) {
        Ok(Check::Absent) => Removal::Absent,
        Ok(Check::Changed) => Removal::Kept,
        Ok(Check::Own) => match crate::platform::fs::safe_remove(path) {
            Ok(()) => Removal::Removed,
            Err(error) => Removal::Failed(error),
        },
        Err(error) => Removal::Failed(error),
    }
}

/// Say, once per target, which outputs Mars left because they changed after it wrote them.
pub(crate) fn report_kept(target_root: &str, dest_path: &str, diag: &mut DiagnosticCollector) {
    diag.warn_each(
        ItemGroup {
            code: "output-kept",
            subject: target_root,
            describe: describe_kept,
        },
        format!("`{dest_path}`"),
    );
}

fn describe_kept(target: &str, count: usize, names: &str) -> String {
    let (noun, tracks, theirs) = if count == 1 {
        ("output", "it, so it is yours", "it")
    } else {
        ("outputs", "them, so they are yours", "them")
    };
    format!(
        "left {count} {noun} in `{target}` that changed after Mars installed {theirs}: {names}. \
         Mars no longer tracks {tracks} to keep or delete"
    )
}

/// What `remove_served` would do to `served`, by `(target root, dest path)` in root then path
/// order: the outputs it would delete (still there, and what Mars wrote, including the
/// `.mars/<surface>/<target>/` copies of per-target items) and the ones it would keep.
pub(crate) struct RemovalPreview {
    pub removed: Vec<(String, String)>,
    pub kept: Vec<(String, String)>,
}

pub(crate) fn preview_removal(project_root: &Path, served: &LockFile) -> RemovalPreview {
    let mut removed = BTreeSet::new();
    let mut kept = BTreeSet::new();
    for item in served.items.values() {
        for record in &item.outputs {
            let path = project_root
                .join(&record.target_root)
                .join(record.dest_path.as_str());
            let pair = (record.target_root.clone(), record.dest_path.to_string());
            match check(&path, item.kind, record) {
                Ok(Check::Own) => removed.insert(pair),
                Ok(Check::Changed) => kept.insert(pair),
                _ => false,
            };
        }
    }
    RemovalPreview {
        removed: removed.into_iter().collect(),
        kept: kept.into_iter().collect(),
    }
}

/// What `remove_served` did: the `(target root, dest path)` of each output it deleted, by
/// root then path, and what it has to say.
pub(crate) struct Removed {
    pub outputs: Vec<(String, String)>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Delete what `served` records (files, hook and MCP entries, the empty directories they leave)
/// and put back into `lock` any record whose removal could not be confirmed: an output that
/// failed to delete (as a pending deletion, retried by the next run), or a hook or MCP entry the
/// user edited. An output that changed since Mars wrote it stays and loses its record
/// (`remove_recorded`). `served` is the half of the lock `LockFile::split_targets` set aside; a
/// crash before the caller writes `lock` leaves the records, and the next run removes the rest.
pub(crate) fn remove_served(
    project_root: &Path,
    lock: &mut LockFile,
    mut served: LockFile,
) -> Result<Removed, MarsError> {
    let targets = served.recorded_targets();
    let mut diagnostics = DiagnosticCollector::new();

    // A directory's contents go before it.
    let mut outputs: Vec<(String, ItemKind, OutputRecord)> = served
        .items
        .values()
        .flat_map(|item| item.outputs.iter().map(|output| (item.kind, output)))
        .map(|(kind, output)| (output.target_root.clone(), kind, output.clone()))
        .collect();
    outputs.sort_by_key(|(_, _, output)| {
        std::cmp::Reverse(output.dest_path.as_str().matches('/').count())
    });
    let mut failed = BTreeSet::new();
    let mut removed = BTreeSet::new();
    for (root, kind, record) in outputs {
        let root_dir = project_root.join(&root);
        let dest_path = record.dest_path.as_str();
        let path = root_dir.join(dest_path);
        match remove_recorded(&path, kind, &record) {
            Removal::Absent => {}
            Removal::Removed => {
                removed.insert((root.clone(), dest_path.to_string()));
                if let Some(parent) = path.parent() {
                    remove_empty_ancestors(parent, &root_dir)?;
                }
            }
            Removal::Kept => report_kept(&root, dest_path, &mut diagnostics),
            Removal::Failed(error) => {
                diagnostics.warn(
                    "output-remove-failed",
                    format!(
                        "Mars keeps the record of an output it could not remove and retries on the next sync: {error}"
                    ),
                );
                failed.insert((root, dest_path.to_string()));
            }
        }
    }
    let mut unremoved = LockFile::empty();
    unremoved.items = std::mem::take(&mut served.items);
    unremoved.items.retain(|_, item| {
        item.outputs.retain_mut(|output| {
            let failed =
                failed.contains(&(output.target_root.clone(), output.dest_path.to_string()));
            if failed {
                output.mark_pending_deletion();
            }
            failed
        });
        !item.outputs.is_empty()
    });
    lock.join(unremoved);

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
    Ok(Removed {
        outputs: removed.into_iter().collect(),
        diagnostics: diagnostics.drain(),
    })
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
