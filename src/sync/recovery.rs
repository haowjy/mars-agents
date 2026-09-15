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
    types::{ContentHash, ItemKind},
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
    items: BTreeMap<String, Vec<LockedItemV2>>,
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
    if intent.version != 1 {
        return Err(invalid(format!(
            "unsupported intent version {}",
            intent.version
        )));
    }
    for versions in intent.items.values() {
        if versions.is_empty() || versions.len() > 2 {
            return Err(invalid("expected current and/or planned output version"));
        }
        for item in versions {
            let [output] = item.outputs.as_slice() else {
                return Err(invalid(
                    "expected exactly one canonical output per intent item",
                ));
            };
            let prefix = match item.kind {
                ItemKind::Agent => "agents/",
                ItemKind::Skill => "skills/",
                ItemKind::Hook => "hooks/",
                ItemKind::McpServer => "mcp/",
                ItemKind::BootstrapDoc => "bootstrap/",
            };
            if output.target_root != CANONICAL_TARGET_ROOT
                || !output.dest_path.as_str().starts_with(prefix)
                || output.installed_checksum().is_none()
                || item.kind != versions[0].kind
                || output.dest_path != versions[0].outputs[0].dest_path
            {
                return Err(invalid("invalid canonical write intent"));
            }
        }
    }
    Ok(Some(intent))
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
    for (key, versions) in intent.items {
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
        let mut item = matching
            .expect("matching regular output checked above")
            .clone();
        if let Some(previous) = old_lock.items.get(&key) {
            item.outputs.extend(
                previous
                    .outputs
                    .iter()
                    .filter(|out| out.target_root != CANONICAL_TARGET_ROOT)
                    .cloned(),
            );
        }
        recovered.push((key, item));
    }
    let count = recovered.len();
    old_lock.items.extend(recovered);
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
    let mut items: BTreeMap<String, Vec<LockedItemV2>> = BTreeMap::new();
    if let Some(previous) = previous.filter(|intent| intent.lock_checksum == checksum) {
        for (key, versions) in previous.items {
            if output_exists(root, &output_path(root, &versions[0]))? {
                // recover() verified this output before resolution. Preserve only
                // that version, not an unattempted version from the previous plan.
                if let Some(item) = old_lock.items.get(&key) {
                    let mut item = item.clone();
                    item.outputs
                        .retain(|output| output.target_root == CANONICAL_TARGET_ROOT);
                    items.insert(key, vec![item]);
                }
            }
        }
    }
    for action in &plan.actions {
        let (PlannedAction::Install { target } | PlannedAction::Overwrite { target }) = action
        else {
            continue;
        };
        let key = lock::item_key(&target.id);
        if index.contains_installed_output(CANONICAL_TARGET_ROOT, &target.dest_path)
            && !items.contains_key(&key)
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
        if path.symlink_metadata().is_ok() && !items.contains_key(&key) {
            // Do not turn a force-adopted dependency collision into crash-recovery
            // evidence. Its preexisting bytes were not written by this transaction.
            continue;
        }
        output_exists(root, &path)?; // Reject ancestor symlinks before recording intent.
        let versions = items.entry(key).or_default();
        if !versions.contains(&item) {
            versions.push(item);
        }
    }
    if !items.is_empty() {
        let intent = WriteIntent {
            version: 1,
            lock_checksum: checksum,
            items,
        };
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
