//! Write-ahead intent for new canonical outputs. A failed apply must not leave
//! completed writes indistinguishable from arbitrary unowned user content.
//!
//! Read under the sync flock and recover into memory. Retain evidence for
//! completed writes when extending intent on a retry; publish installed ownership
//! only at finalization, including when repair is preserving a corrupt lock.

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::plan::{PlannedAction, SyncPlan};
use crate::{
    error::{LockError, MarsError},
    hash,
    lock::{self, CANONICAL_TARGET_ROOT, LockFile, LockIndex, LockedItemV2, OutputRecord},
    resolve::ResolvedGraph,
    types::{ContentHash, DestPath, ItemKind},
};

const INTENT_FILE: &str = ".mars/pending-canonical.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteIntent {
    version: u32,
    /// Bind recovery to the lock that authorized this plan, not a different
    /// checkout's lock or a subsequently replaced ownership registry.
    lock_checksum: Option<ContentHash>,
    // A retry may overwrite a recovered output. Keep both the verified current
    // version and the planned version until finalization, covering death on either
    // side of that write without prematurely replacing mars.lock.
    outputs: BTreeMap<DestPath, Vec<LockedItemV2>>,
}

fn invalid(message: impl std::fmt::Display) -> MarsError {
    LockError::Corrupt {
        message: format!("{INTENT_FILE}: {message}; preserve recovery evidence and relocate any conflicting output before retrying"),
    }.into()
}

fn lock_checksum(root: &Path) -> Result<Option<ContentHash>, MarsError> {
    match fs::read(root.join("mars.lock")) {
        Ok(bytes) => Ok(Some(hash::hash_bytes(&bytes).into())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read(root: &Path) -> Result<Option<WriteIntent>, MarsError> {
    let path = root.join(INTENT_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(invalid("write intent is not a regular file"));
        }
        Ok(_) => {}
    }
    if root
        .join(".mars")
        .symlink_metadata()?
        .file_type()
        .is_symlink()
    {
        return Err(invalid("canonical root is a symlink"));
    }
    let intent: WriteIntent = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| invalid(format!("cannot read write intent: {error}")))?;
    validate(&intent)?;
    Ok(Some(intent))
}

fn logical_key(item: &LockedItemV2) -> Result<String, MarsError> {
    let dest = &item.outputs[0].dest_path;
    let name = dest.item_name(item.kind);
    if item.kind == ItemKind::Hook {
        let Some((target, _)) = super::target::hook_target_dest_path(dest) else {
            return Err(invalid("invalid target-scoped hook destination"));
        };
        Ok(format!("hook/{name}@{target}"))
    } else {
        Ok(format!("{}/{name}", item.kind))
    }
}

/// The journal is indexed by physical output, not logical item. One unfinished
/// item move can leave several paths, each with current/planned byte versions.
fn validate(intent: &WriteIntent) -> Result<(), MarsError> {
    if intent.version != 1 {
        return Err(invalid(format!(
            "unsupported intent version {}",
            intent.version
        )));
    }
    for (dest, versions) in &intent.outputs {
        if versions.is_empty() || versions.len() > 2 {
            return Err(invalid("expected current and/or planned output version"));
        }
        for item in versions {
            let [output] = item.outputs.as_slice() else {
                return Err(invalid(
                    "expected exactly one canonical output per intent item",
                ));
            };
            if output.target_root != CANONICAL_TARGET_ROOT
                || &output.dest_path != dest
                || (item.kind == ItemKind::BootstrapDoc
                    && !dest.as_str().ends_with("/BOOTSTRAP.md"))
                || output.installed_checksum().is_none()
                || item.kind != versions[0].kind
            {
                return Err(invalid("invalid canonical write intent"));
            }
            logical_key(item)?;
            validate_destination(dest, item.kind)?;
        }
    }
    Ok(())
}

fn validate_destination(dest: &DestPath, kind: ItemKind) -> Result<(), MarsError> {
    let path = Path::new(dest.as_str());
    let output = if kind == ItemKind::BootstrapDoc {
        path.parent().unwrap_or(Path::new(""))
    } else {
        path
    };
    let journal = Path::new(INTENT_FILE)
        .strip_prefix(CANONICAL_TARGET_ROOT)
        .expect("journal is canonical metadata");
    // Reserve case and trailing-dot/space aliases on every platform. A checkout
    // must not gain a metadata collision when moved to a case-insensitive or
    // Win32 filesystem, even if that spelling is distinct on the current host.
    let aliases_journal = output.components().next().is_some_and(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(&journal.to_string_lossy())
    });
    if aliases_journal || journal.starts_with(output) {
        return Err(MarsError::InvalidRequest {
            message: format!(
                "canonical destination {dest} overlaps reserved recovery state {INTENT_FILE}; choose a different destination"
            ),
        });
    }
    Ok(())
}

/// Check every action before config or output mutations, including dry runs.
pub(super) fn validate_plan(plan: &SyncPlan) -> Result<(), MarsError> {
    for action in &plan.actions {
        let (dest, kind) = match action {
            PlannedAction::Install { target } | PlannedAction::Overwrite { target } => {
                (&target.dest_path, target.id.kind)
            }
            PlannedAction::Remove { locked } => (&locked.dest_path, locked.kind),
            PlannedAction::Skip {
                item_id, dest_path, ..
            }
            | PlannedAction::KeepLocal {
                item_id, dest_path, ..
            } => (dest_path, item_id.kind),
        };
        validate_destination(dest, kind)?;
    }
    Ok(())
}

fn output_path(root: &Path, item: &LockedItemV2) -> PathBuf {
    let path = item.outputs[0]
        .dest_path
        .resolve(&root.join(CANONICAL_TARGET_ROOT));
    if item.kind == ItemKind::BootstrapDoc {
        path.parent()
            .expect("bootstrap path has a directory")
            .to_path_buf()
    } else {
        path
    }
}

/// Inspect the entire output path without following ancestor symlinks. Missing
/// parents (including a non-directory obstruction) mean no output was installed.
fn output_exists(root: &Path, path: &Path) -> Result<bool, MarsError> {
    let mut current = root.to_path_buf();
    for component in path
        .strip_prefix(root)
        .expect("canonical output is under project root")
        .components()
    {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(invalid(format!(
                    "refusing symlink at {}",
                    current.display()
                )));
            }
            Ok(meta) if current != path && !meta.is_dir() => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

/// Recover only outputs whose pre-write intent still belongs to this lock and
/// whose bytes match. The caller keeps all changes in memory until preflight.
pub(super) fn recover(root: &Path, old_lock: &mut LockFile) -> Result<usize, MarsError> {
    let Some(intent) = read(root)? else {
        return Ok(0);
    };
    let same_lock = intent.lock_checksum == lock_checksum(root)?;
    let index = LockIndex::new(old_lock);
    let mut recovered = Vec::new();
    for versions in intent.outputs.into_values() {
        let item = &versions[0];
        let output = &item.outputs[0];
        // Handles interruption after final lock publication but before intent
        // cleanup. Already-published ownership takes precedence over old intent.
        if index.contains_installed_output(CANONICAL_TARGET_ROOT, &output.dest_path) {
            continue;
        }
        let path = output_path(root, item);
        if !output_exists(root, &path)? {
            continue;
        }
        if !same_lock {
            return Err(invalid(format!(
                "mars.lock changed since the pending write to {}",
                path.display()
            )));
        }
        let metadata = fs::symlink_metadata(&path)?;
        let expects_file = matches!(item.kind, ItemKind::Agent | ItemKind::McpServer);
        let checksum = lock::regular_output_checksum(&path);
        let matching = versions.iter().rev().find(|candidate| {
            candidate.outputs[0]
                .installed_checksum()
                .map(|hash| hash.as_ref())
                == checksum.as_deref()
        });
        if (expects_file && !metadata.is_file())
            || (!expects_file && !metadata.is_dir())
            || matching.is_none()
        {
            return Err(invalid(format!(
                "pending output {} does not match its recorded bytes or contains non-regular entries",
                path.display()
            )));
        }
        let item = matching
            .expect("matching regular output checked above")
            .clone();
        recovered.push((logical_key(&item)?, item));
    }
    let count = recovered.len();
    for (key, mut item) in recovered {
        let dest = item.outputs[0].dest_path.clone();
        if let Some(previous) = old_lock.items.get(&key) {
            // Merge each recovered path, not just the final record for this item.
            // Old canonical claims survive until removal is positively confirmed.
            item.outputs.extend(
                previous
                    .outputs
                    .iter()
                    .filter(|output| {
                        output.target_root != CANONICAL_TARGET_ROOT || output.dest_path != dest
                    })
                    .cloned(),
            );
        }
        old_lock.items.insert(key, item);
    }
    Ok(count)
}

/// Publish intent before apply, retaining the verified current version of any
/// previously recovered write. The lock stays untouched until finalization.
pub(super) fn prepare(
    root: &Path,
    plan: &SyncPlan,
    graph: &ResolvedGraph,
    old_lock: &LockFile,
) -> Result<(), MarsError> {
    let previous = read(root)?;
    let checksum = lock_checksum(root)?;
    let index = LockIndex::new(old_lock);
    let mut outputs = BTreeMap::new();
    if let Some(previous) = previous.filter(|intent| intent.lock_checksum == checksum) {
        for (dest, versions) in previous.outputs {
            if output_exists(root, &output_path(root, &versions[0]))? {
                // Keep the exact provenance of this verified physical output,
                // not another path's provenance from the merged logical item.
                if let Some(installed) = index.find_output(CANONICAL_TARGET_ROOT, &dest)
                    && let Some(item) = versions.into_iter().rev().find(|item| {
                        item.outputs[0].installed_checksum() == Some(&installed.installed_checksum)
                    })
                {
                    outputs.insert(dest, vec![item]);
                }
            }
        }
    }
    for action in &plan.actions {
        let (PlannedAction::Install { target } | PlannedAction::Overwrite { target }) = action
        else {
            continue;
        };
        let dest = &target.dest_path;
        if index.contains_installed_output(CANONICAL_TARGET_ROOT, &target.dest_path)
            && !outputs.contains_key(dest)
        {
            continue;
        }
        let expected = target
            .rewritten_content
            .as_ref()
            .map(|content| ContentHash::from(hash::hash_bytes(content.as_bytes())))
            .unwrap_or_else(|| target.source_hash.clone());
        let item = LockedItemV2 {
            source: target.source_name.clone(),
            kind: target.id.kind,
            version: graph
                .nodes
                .get(&target.source_name)
                .and_then(|node| node.resolved_ref.version_tag.clone()),
            source_checksum: target.source_hash.clone(),
            outputs: vec![OutputRecord::installed(
                CANONICAL_TARGET_ROOT.to_string(),
                target.dest_path.clone(),
                expected,
            )],
        };
        let path = output_path(root, &item);
        if path.symlink_metadata().is_ok() && !outputs.contains_key(dest) {
            // Do not turn a force-adopted dependency collision into crash-recovery
            // evidence. Its preexisting bytes were not written by this transaction.
            continue;
        }
        output_exists(root, &path)?; // Reject ancestor symlinks before recording intent.
        let versions = outputs.entry(dest.clone()).or_default();
        if !versions.contains(&item) {
            versions.push(item);
        }
    }
    if !outputs.is_empty() {
        let intent = WriteIntent {
            version: 1,
            lock_checksum: checksum,
            outputs,
        };
        validate(&intent)?;
        let bytes = serde_json::to_vec_pretty(&intent).map_err(invalid)?;
        crate::fs::atomic_write_if_changed(&root.join(INTENT_FILE), &bytes)?;
    } else {
        complete(root)?;
    }
    Ok(())
}

/// Discard after lock publication, or when no uncommitted outputs remain.
/// A crash after publication is harmless: the installed lock claims take precedence.
pub(super) fn complete(root: &Path) -> Result<(), MarsError> {
    match fs::remove_file(root.join(INTENT_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
