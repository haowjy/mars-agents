//! Target sync — copy content from .mars/ canonical store to managed targets.
//!
//! After `apply_plan()` writes resolved content to `.mars/agents/` and `.mars/skills/`,
//! this module copies that content to all configured native target directories (`.claude/`, etc.).
//!
//! All targets are managed outputs — they get copies (not symlinks) of .mars/ content.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::LockFile;
use crate::reconcile::fs_ops;
use crate::surface_ownership::{self, CollisionAdoptHint, SurfaceCopyDecision};
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::ContentHash;
use crate::types::managed_cmd;

/// A directory that mars manages — materialized from .mars/.
#[derive(Debug, Clone)]
pub struct ManagedTarget {
    /// Target directory path relative to project root (e.g. ".claude").
    pub path: String,
}

/// A linked-target output recorded during sync for lock persistence.
#[derive(Debug, Clone)]
pub struct TargetSyncedOutput {
    pub dest_path: String,
    pub installed_checksum: ContentHash,
}

/// Result of syncing content to a single target directory.
#[derive(Debug, Clone)]
pub struct TargetSyncOutcome {
    /// Target directory name (e.g. ".claude").
    pub target: String,
    /// Number of items successfully synced.
    pub items_synced: usize,
    /// Number of items removed (orphan cleanup).
    pub items_removed: usize,
    /// Non-fatal errors encountered during sync.
    pub errors: Vec<String>,
    /// Outputs successfully copied to this target (for lock persistence).
    pub synced_outputs: Vec<TargetSyncedOutput>,
    /// Dest paths removed from this target (for lock persistence).
    pub removed_dest_paths: Vec<String>,
}

/// Per-run target sync options shared across all linked targets.
pub struct TargetSyncContext<'a> {
    pub old_lock: &'a LockFile,
    pub force: bool,
    pub collision_hint: CollisionAdoptHint,
    /// Managed native agent paths to exempt from orphan cleanup (selective mode).
    pub orphan_preserve_paths: Option<&'a HashMap<String, HashSet<String>>>,
}

/// Sync all managed targets from .mars/ canonical store.
///
/// For each configured target, copies content from `.mars/agents/` and `.mars/skills/`
/// into the target directory.
/// Cleans up orphaned items that are no longer in the apply outcomes.
///
/// Target sync is non-fatal by default (D9) — errors per-target are recorded but don't
/// stop other targets from being synced.
pub fn sync_managed_targets(
    project_root: &Path,
    mars_dir: &Path,
    targets: &[String],
    outcomes: &[ActionOutcome],
    ctx: &TargetSyncContext<'_>,
    diag: &mut DiagnosticCollector,
) -> Vec<TargetSyncOutcome> {
    let mut results = Vec::new();

    for target_name in targets {
        let target_root = project_root.join(target_name);
        match sync_one_target(mars_dir, &target_root, target_name, outcomes, ctx, diag) {
            Ok(outcome) => {
                if !outcome.errors.is_empty() {
                    for err in &outcome.errors {
                        diag.warn(
                            "target-sync-error",
                            format!("target `{target_name}`: {err}"),
                        );
                    }
                }
                results.push(outcome);
            }
            Err(e) => {
                diag.warn(
                    "target-sync-failed",
                    format!("target `{target_name}` sync failed: {e}"),
                );
                results.push(TargetSyncOutcome {
                    target: target_name.clone(),
                    items_synced: 0,
                    items_removed: 0,
                    errors: vec![e.to_string()],
                    synced_outputs: Vec::new(),
                    removed_dest_paths: Vec::new(),
                });
            }
        }
    }

    results
}

fn sync_one_target(
    mars_dir: &Path,
    target_root: &Path,
    target_name: &str,
    outcomes: &[ActionOutcome],
    ctx: &TargetSyncContext<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<TargetSyncOutcome, MarsError> {
    let old_lock = ctx.old_lock;
    let force = ctx.force;
    let collision_hint = ctx.collision_hint;
    let mut items_synced = 0;
    let mut items_removed = 0;
    let mut errors = Vec::new();
    let mut synced_outputs = Vec::new();
    let mut removed_dest_paths = Vec::new();
    let previous_managed_paths = old_lock.output_dest_paths_for_target(target_name);

    std::fs::create_dir_all(target_root)?;

    let mut expected_paths: HashSet<String> = HashSet::new();
    let target_registry = crate::target::TargetRegistry::new();
    let target_adapter = target_registry.get(target_name);
    let native_skill_variant_key = target_adapter
        .and_then(|adapter| adapter.skill_variant_key())
        .map(str::to_owned);
    let target_accepts_canonical_agents = target_adapter
        .map(|adapter| {
            adapter
                .default_dest_path(crate::lock::ItemKind::Agent, "__mars_probe__")
                .is_some()
        })
        .unwrap_or(true);

    for outcome in outcomes {
        if outcome.item_id.kind == crate::lock::ItemKind::BootstrapDoc {
            continue;
        }
        let dest_rel = outcome.dest_path.as_str();
        if outcome.item_id.kind == crate::lock::ItemKind::Agent && !target_accepts_canonical_agents
        {
            if matches!(outcome.action, ActionTaken::Removed) {
                let target_path = target_root.join(dest_rel);
                if remove_target_path_if_managed(
                    &target_path,
                    target_name,
                    dest_rel,
                    old_lock,
                    &mut errors,
                ) {
                    items_removed += 1;
                    removed_dest_paths.push(dest_rel.to_string());
                }
            }
            continue;
        }
        match &outcome.action {
            ActionTaken::Removed => {
                let target_path = target_root.join(dest_rel);
                if remove_target_path_if_managed(
                    &target_path,
                    target_name,
                    dest_rel,
                    old_lock,
                    &mut errors,
                ) {
                    items_removed += 1;
                    removed_dest_paths.push(dest_rel.to_string());
                }
            }
            ActionTaken::Skipped => {
                expected_paths.insert(dest_rel.to_string());
                let source = mars_dir.join(dest_rel);
                let dest = target_root.join(dest_rel);
                if source.exists() || source.symlink_metadata().is_ok() {
                    let should_refresh_native_skill = outcome.item_id.kind
                        == crate::lock::ItemKind::Skill
                        && native_skill_variant_key.is_some();
                    let dest_exists = surface_ownership::target_dest_exists(&dest);
                    let wants_copy = force || !dest_exists || should_refresh_native_skill;
                    if wants_copy {
                        if should_copy_to_target(
                            &dest,
                            target_name,
                            dest_rel,
                            old_lock,
                            force,
                            collision_hint,
                            diag,
                        ) {
                            let previous_target_hash = if should_refresh_native_skill && dest_exists
                            {
                                crate::hash::compute_hash(&dest, outcome.item_id.kind).ok()
                            } else {
                                None
                            };
                            match copy_item_to_target(
                                &source,
                                &dest,
                                outcome.item_id.kind,
                                outcome.item_id.name.as_str(),
                                native_skill_variant_key.as_deref(),
                                diag,
                            ) {
                                Ok(true) => {
                                    items_synced += 1;
                                    record_synced_output(
                                        &mut synced_outputs,
                                        &dest,
                                        dest_rel,
                                        outcome.item_id.kind,
                                    );
                                    if let Some(previous_target_hash) = previous_target_hash
                                        && let Ok(current_target_hash) =
                                            crate::hash::compute_hash(&dest, outcome.item_id.kind)
                                        && previous_target_hash != current_target_hash
                                    {
                                        diag.warn(
                                            "target-native-projection-repaired",
                                            format!(
                                                "repaired diverged native projection: {target_name}/{dest_rel}/SKILL.md"
                                            ),
                                        );
                                    }
                                }
                                Ok(false) => {}
                                Err(e) => errors.push(format!("failed to copy {dest_rel}: {e}")),
                            }
                        }
                    } else if native_skill_variant_key.is_none()
                        && old_lock.contains_output(target_name, dest_rel)
                        && let Some(expected_checksum) = &outcome.installed_checksum
                    {
                        match crate::hash::compute_hash(&dest, outcome.item_id.kind) {
                            Ok(actual) => {
                                let actual = ContentHash::from(actual);
                                if &actual != expected_checksum {
                                    diag.warn(
                                        "target-divergent",
                                        format!(
                                            "target `{target_name}` item `{}` diverged from `.mars` (preserved local content; run `{cmd1}` or `{cmd2}` to reset)",
                                            dest_rel,
                                            cmd1 = managed_cmd("mars sync --force"),
                                            cmd2 = managed_cmd("mars repair"),
                                        ),
                                    );
                                }
                            }
                            Err(e) => {
                                errors.push(format!("failed to verify {dest_rel} checksum: {e}"))
                            }
                        }
                    } else if dest_exists && !old_lock.contains_output(target_name, dest_rel) {
                        surface_ownership::warn_unmanaged_collision(
                            target_name,
                            dest_rel,
                            collision_hint,
                            diag,
                        );
                    }
                }
            }
            _ => {
                expected_paths.insert(dest_rel.to_string());
                let source = mars_dir.join(dest_rel);
                let dest = target_root.join(dest_rel);
                if (source.exists() || source.symlink_metadata().is_ok())
                    && should_copy_to_target(
                        &dest,
                        target_name,
                        dest_rel,
                        old_lock,
                        force,
                        collision_hint,
                        diag,
                    )
                {
                    match copy_item_to_target(
                        &source,
                        &dest,
                        outcome.item_id.kind,
                        outcome.item_id.name.as_str(),
                        native_skill_variant_key.as_deref(),
                        diag,
                    ) {
                        Ok(true) => {
                            items_synced += 1;
                            record_synced_output(
                                &mut synced_outputs,
                                &dest,
                                dest_rel,
                                outcome.item_id.kind,
                            );
                        }
                        Ok(false) => {}
                        Err(e) => errors.push(format!("failed to copy {dest_rel}: {e}")),
                    }
                }
            }
        }
    }

    if let Some(preserve) = ctx.orphan_preserve_paths
        && let Some(paths) = preserve.get(target_name)
    {
        expected_paths.extend(paths.iter().cloned());
    }

    let orphan_removed = cleanup_orphans(
        target_root,
        &expected_paths,
        &previous_managed_paths,
        &mut removed_dest_paths,
        &mut errors,
    );
    items_removed += orphan_removed;

    Ok(TargetSyncOutcome {
        target: target_name.to_string(),
        items_synced,
        items_removed,
        errors,
        synced_outputs,
        removed_dest_paths,
    })
}

fn should_copy_to_target(
    dest: &Path,
    target_name: &str,
    dest_rel: &str,
    old_lock: &LockFile,
    force: bool,
    collision_hint: CollisionAdoptHint,
    diag: &mut DiagnosticCollector,
) -> bool {
    let dest_exists = surface_ownership::target_dest_exists(dest);
    match surface_ownership::copy_decision(old_lock, target_name, dest_rel, dest_exists, force) {
        SurfaceCopyDecision::Proceed => {
            if dest_exists && force && !old_lock.contains_output(target_name, dest_rel) {
                surface_ownership::warn_unmanaged_adopted(
                    target_name,
                    dest_rel,
                    collision_hint,
                    diag,
                );
            }
            true
        }
        SurfaceCopyDecision::SkipUnmanagedCollision => {
            surface_ownership::warn_unmanaged_collision(
                target_name,
                dest_rel,
                collision_hint,
                diag,
            );
            false
        }
    }
}

fn remove_target_path_if_managed(
    target_path: &Path,
    target_name: &str,
    dest_rel: &str,
    old_lock: &LockFile,
    errors: &mut Vec<String>,
) -> bool {
    if !surface_ownership::target_dest_exists(target_path) {
        return false;
    }
    if !surface_ownership::may_delete(old_lock, target_name, dest_rel) {
        return false;
    }
    match fs_ops::safe_remove(target_path) {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("failed to remove {dest_rel}: {e}"));
            false
        }
    }
}

fn record_synced_output(
    synced_outputs: &mut Vec<TargetSyncedOutput>,
    dest: &Path,
    dest_rel: &str,
    kind: crate::lock::ItemKind,
) {
    if let Ok(checksum) = crate::hash::compute_hash(dest, kind) {
        synced_outputs.push(TargetSyncedOutput {
            dest_path: dest_rel.to_string(),
            installed_checksum: ContentHash::from(checksum),
        });
    }
}

/// Copy an item (file or directory) from .mars/ to a target directory.
///
/// Follows symlinks on the source side (D26 — targets get file copies, not symlinks).
/// Uses atomic operations via the reconcile layer.
///
/// Returns `true` when bytes were written to `dest`, `false` when existing content
/// was already byte-identical and left untouched.
fn copy_item_to_target(
    source: &Path,
    dest: &Path,
    kind: crate::lock::ItemKind,
    item_name: &str,
    native_skill_variant_key: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    if kind == crate::lock::ItemKind::Skill && native_skill_variant_key.is_some() {
        crate::compiler::variants::validate_skill_variants(source, item_name, diag);
        return crate::compiler::variants::project_skill_for_target(
            source,
            dest,
            native_skill_variant_key,
            diag,
            item_name,
        );
    }

    // Ensure parent directories exist
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Follow symlinks to determine if source is a file or directory
    let metadata = std::fs::metadata(source)?;

    if metadata.is_dir() {
        if dest.exists() && fs_ops::directory_trees_content_equal(source, dest)? {
            return Ok(false);
        }
        fs_ops::atomic_copy_dir(source, dest)?;
    } else if metadata.is_file() {
        if fs_ops::file_content_equal(source, dest)? {
            return Ok(false);
        }
        fs_ops::atomic_copy_file(source, dest)?;
    }

    Ok(true)
}

/// Clean up orphaned items in a target directory.
///
/// Uses lock v2 output records (via `previous_managed_paths`) to determine
/// what was managed in the prior sync, rather than scanning hardcoded
/// subdirectories. Removes entries that were previously managed but are no
/// longer expected in the current sync.
///
/// Returns the number of items removed.
fn cleanup_orphans(
    target_root: &Path,
    expected: &HashSet<String>,
    previous_managed_paths: &HashSet<String>,
    removed_dest_paths: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> usize {
    let mut removed = 0;

    // Lock-driven: iterate paths from the old lock, not hardcoded subdirectories.
    // Only remove entries that were previously managed and are no longer expected.
    for managed_path in previous_managed_paths {
        if expected.contains(managed_path) {
            continue;
        }

        let full_path = target_root.join(managed_path);

        // Skip if the path doesn't exist (already removed or never synced to this target).
        if !full_path.exists() && full_path.symlink_metadata().is_err() {
            continue;
        }

        // Skip symlinked paths (legacy link setup — don't touch).
        if full_path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }

        if let Err(e) = fs_ops::safe_remove(&full_path) {
            errors.push(format!("failed to remove orphan {managed_path}: {e}"));
        } else {
            removed += 1;
            removed_dest_paths.push(managed_path.clone());
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticCollector;
    use crate::hash;
    use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
    use crate::surface_ownership::CollisionAdoptHint;
    use crate::sync::apply::{ActionOutcome, ActionTaken};
    use crate::types::{DestPath, ItemName};
    use tempfile::TempDir;

    fn make_outcome(dest: &str, action: ActionTaken) -> ActionOutcome {
        ActionOutcome {
            item_id: crate::lock::ItemId {
                kind: crate::lock::ItemKind::Agent,
                name: ItemName::from("test"),
            },
            action,
            dest_path: DestPath::from(dest),
            source_name: "test-source".into(),
            source_checksum: None,
            installed_checksum: None,
        }
    }

    fn lock_with_target_outputs(target: &str, outputs: &[(&str, &str)]) -> LockFile {
        let mut lock = LockFile::empty();
        for (dest, checksum) in outputs {
            let name = dest.rsplit('/').next().unwrap_or("item");
            lock.items.insert(
                format!("agent/{name}"),
                LockedItemV2 {
                    source: "test".into(),
                    kind: ItemKind::Agent,
                    version: None,
                    source_checksum: "sha256:src".into(),
                    outputs: vec![OutputRecord {
                        target_root: target.to_string(),
                        dest_path: (*dest).into(),
                        installed_checksum: (*checksum).into(),
                    }],
                },
            );
        }
        lock
    }

    fn lock_with_skill_target_outputs(target: &str, outputs: &[(&str, &str)]) -> LockFile {
        let mut lock = LockFile::empty();
        for (dest, checksum) in outputs {
            let name = dest.rsplit('/').next().unwrap_or("item");
            lock.items.insert(
                format!("skill/{name}"),
                LockedItemV2 {
                    source: "test".into(),
                    kind: ItemKind::Skill,
                    version: None,
                    source_checksum: "sha256:src".into(),
                    outputs: vec![OutputRecord {
                        target_root: target.to_string(),
                        dest_path: (*dest).into(),
                        installed_checksum: (*checksum).into(),
                    }],
                },
            );
        }
        lock
    }

    fn target_sync_ctx<'a>(old_lock: &'a LockFile, force: bool) -> TargetSyncContext<'a> {
        TargetSyncContext {
            old_lock,
            force,
            collision_hint: CollisionAdoptHint::SyncForce,
            orphan_preserve_paths: None,
        }
    }

    fn make_skipped_with_checksum(dest: &str, checksum: &str) -> ActionOutcome {
        let mut outcome = make_outcome(dest, ActionTaken::Skipped);
        outcome.installed_checksum = Some(checksum.into());
        outcome
    }

    #[test]
    fn sync_copies_installed_items_to_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        // Set up .mars/ content
        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].items_synced, 1);
        assert!(results[0].errors.is_empty());
        assert!(target.join("agents/coder.md").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("agents/coder.md")).unwrap(),
            "# Coder"
        );
    }

    #[test]
    fn sync_removes_items_from_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(&mars_dir).unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/old.md"), "# Old").unwrap();

        let outcomes = vec![make_outcome("agents/old.md", ActionTaken::Removed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/old.md", "sha256:old")]),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_removed, 1);
        assert!(!target.join("agents/old.md").exists());
    }

    #[test]
    fn sync_cleans_up_previous_managed_orphans() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        // Set up .mars/ with one agent
        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        // Set up target with an extra agent (orphan)
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/orphan.md"), "# Orphan").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/orphan.md", "sha256:orphan")]),
                false,
            ),
            &mut diag,
        );

        assert!(target.join("agents/coder.md").exists());
        assert!(!target.join("agents/orphan.md").exists());
        assert_eq!(results[0].items_removed, 1);
    }

    #[test]
    fn sync_preserves_unmanaged_files_in_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/custom.md"), "# User custom").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert!(target.join("agents/coder.md").exists());
        assert!(target.join("agents/custom.md").exists());
        assert_eq!(results[0].items_removed, 0);
    }

    #[test]
    fn sync_removed_agent_outcome_removes_existing_target_agent_without_copying() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# Existing target copy").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Removed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:coder")]),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 0);
        assert_eq!(results[0].items_removed, 1);
        assert!(!target.join("agents/coder.md").exists());
        assert!(results[0].errors.is_empty());
    }

    #[test]
    fn selective_orphan_preserve_keeps_native_agent_without_agent_outcomes() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".claude");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# Native").unwrap();

        let old_lock = lock_with_target_outputs(".claude", &[("agents/coder.md", "sha256:native")]);
        let mut preserve = HashMap::new();
        preserve.insert(
            ".claude".to_string(),
            HashSet::from(["agents/coder.md".to_string()]),
        );
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &[],
            &TargetSyncContext {
                old_lock: &old_lock,
                force: false,
                collision_hint: CollisionAdoptHint::SyncForce,
                orphan_preserve_paths: Some(&preserve),
            },
            &mut diag,
        );

        assert!(target.join("agents/coder.md").exists());
        assert_eq!(results[0].items_removed, 0);
        assert!(
            !results[0]
                .removed_dest_paths
                .iter()
                .any(|path| path == "agents/coder.md"),
            "selective steady-state must not remove managed native agent before compile"
        );
    }

    #[test]
    fn sync_multiple_targets() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string(), ".custom-target".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert_eq!(results.len(), 2);
        assert!(dir.path().join(".agents/agents/coder.md").exists());
        assert!(dir.path().join(".custom-target/agents/coder.md").exists());
    }

    #[test]
    fn sync_native_targets_skip_canonical_agent_markdown_copies() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[
                ".claude".to_string(),
                ".codex".to_string(),
                ".opencode".to_string(),
                ".pi".to_string(),
            ],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|outcome| outcome.items_synced == 0));
        assert!(!dir.path().join(".claude/agents/coder.md").exists());
        assert!(!dir.path().join(".codex/agents/coder.md").exists());
        assert!(!dir.path().join(".opencode/agents/coder.md").exists());
        assert!(!dir.path().join(".pi/agents/coder.md").exists());
    }

    #[test]
    fn sync_unknown_target_still_copies_canonical_agents() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".custom-target".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert!(dir.path().join(".custom-target/agents/coder.md").exists());
    }

    #[test]
    fn sync_skill_directory() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("skills/planning")).unwrap();
        std::fs::write(mars_dir.join("skills/planning/SKILL.md"), "# Planning").unwrap();

        let mut outcome = make_outcome("skills/planning", ActionTaken::Installed);
        outcome.item_id.kind = crate::lock::ItemKind::Skill;
        let outcomes = vec![outcome];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert!(target.join("skills/planning/SKILL.md").exists());
    }

    #[test]
    fn sync_projects_skills_for_native_harness_targets() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".claude");

        std::fs::create_dir_all(mars_dir.join("skills/planning/resources")).unwrap();
        std::fs::create_dir_all(mars_dir.join("skills/planning/variants/claude")).unwrap();
        std::fs::create_dir_all(target.join("skills")).unwrap();
        std::fs::write(target.join("skills/orphan"), "# Orphan").unwrap();
        std::fs::write(mars_dir.join("skills/planning/SKILL.md"), "# Base").unwrap();
        std::fs::write(
            mars_dir.join("skills/planning/resources/BOOTSTRAP.md"),
            "# Bootstrap",
        )
        .unwrap();
        std::fs::write(
            mars_dir.join("skills/planning/variants/claude/SKILL.md"),
            "# Claude",
        )
        .unwrap();

        let mut outcome = make_outcome("skills/planning", ActionTaken::Installed);
        outcome.item_id.kind = crate::lock::ItemKind::Skill;
        let outcomes = vec![outcome];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_skill_target_outputs(
                    ".claude",
                    &[
                        ("skills/planning", "sha256:planning"),
                        ("skills/orphan", "sha256:orphan"),
                    ],
                ),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert_eq!(
            std::fs::read_to_string(target.join("skills/planning/SKILL.md")).unwrap(),
            "# Claude"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("skills/planning/resources/BOOTSTRAP.md")).unwrap(),
            "# Bootstrap"
        );
        assert!(!target.join("skills/planning/variants").exists());
        assert!(!target.join("skills/orphan").exists());
    }

    #[test]
    fn cleanup_orphans_uses_forward_slash_keys_for_expected_paths() {
        let dir = TempDir::new().unwrap();
        let target_root = dir.path().join(".agents");
        std::fs::create_dir_all(target_root.join("agents")).unwrap();
        std::fs::write(target_root.join("agents/coder.md"), "# Managed").unwrap();
        std::fs::write(target_root.join("agents/orphan.md"), "# Orphan").unwrap();

        let mut expected = HashSet::new();
        expected.insert(
            DestPath::new(r"agents\coder.md")
                .unwrap()
                .as_str()
                .to_string(),
        );

        let previous = lock_with_target_outputs(
            ".agents",
            &[
                ("agents/coder.md", "sha256:coder"),
                ("agents/orphan.md", "sha256:orphan"),
            ],
        );
        let previous_paths = previous.output_dest_paths_for_target(".agents");
        let mut removed_dest_paths = Vec::new();
        let removed = cleanup_orphans(
            &target_root,
            &expected,
            &previous_paths,
            &mut removed_dest_paths,
            &mut Vec::new(),
        );

        assert_eq!(removed, 1);
        assert!(target_root.join("agents/coder.md").exists());
        assert!(!target_root.join("agents/orphan.md").exists());
    }

    #[test]
    fn sync_convergence_on_rerun() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Coder").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        // First run
        sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        // Second run with Skipped action — should converge (file already exists)
        let outcomes2 = vec![make_outcome("agents/coder.md", ActionTaken::Skipped)];
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes2,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:coder")]),
                false,
            ),
            &mut diag,
        );

        assert!(target.join("agents/coder.md").exists());
        // items_synced should be 0 since file already exists
        assert_eq!(results[0].items_synced, 0);
    }

    #[test]
    fn sync_force_refreshes_skipped_target_content() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();

        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# Tampered").unwrap();

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Skipped)];
        let mut diag = DiagnosticCollector::new();
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:coder")]),
                true,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert_eq!(
            std::fs::read_to_string(target.join("agents/coder.md")).unwrap(),
            "# Canonical"
        );
    }

    #[test]
    fn sync_skipped_recopies_missing_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();

        let checksum = hash::hash_bytes(b"# Canonical");
        let outcomes = vec![make_skipped_with_checksum("agents/coder.md", &checksum)];
        let mut diag = DiagnosticCollector::new();
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:coder")]),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert!(target.join("agents/coder.md").exists());
    }

    #[test]
    fn sync_skipped_warns_on_divergent_target_and_preserves_local_content() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();

        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# Locally edited").unwrap();

        let checksum = hash::hash_bytes(b"# Canonical");
        let outcomes = vec![make_skipped_with_checksum("agents/coder.md", &checksum)];
        let mut diag = DiagnosticCollector::new();
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(
                &lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:coder")]),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 0);
        assert_eq!(
            std::fs::read_to_string(target.join("agents/coder.md")).unwrap(),
            "# Locally edited"
        );

        let diagnostics = diag.drain();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "target-divergent" && d.message.contains("agents/coder.md"))
        );
    }

    #[test]
    fn sync_preserves_handwritten_collision_when_lock_only_tracks_mars() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".cursor");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/design-lead.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/cursor-only-test.md"), "# custom").unwrap();
        std::fs::write(target.join("agents/design-lead.md"), "# hand-written").unwrap();

        let mut lock = LockFile::empty();
        lock.items.insert(
            "agent/design-lead".to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs: vec![OutputRecord {
                    target_root: ".mars".to_string(),
                    dest_path: "agents/design-lead.md".into(),
                    installed_checksum: "sha256:mars".into(),
                }],
            },
        );

        let outcomes = vec![make_outcome("agents/design-lead.md", ActionTaken::Removed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".cursor".to_string()],
            &outcomes,
            &target_sync_ctx(&lock, false),
            &mut diag,
        );

        assert_eq!(results[0].items_removed, 0);
        assert!(target.join("agents/cursor-only-test.md").exists());
        assert!(target.join("agents/design-lead.md").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("agents/design-lead.md")).unwrap(),
            "# hand-written"
        );
    }

    #[test]
    fn sync_installed_does_not_overwrite_untracked_collision_in_linked_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# hand-written").unwrap();

        let mut lock = LockFile::empty();
        lock.items.insert(
            "agent/coder".to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs: vec![OutputRecord {
                    target_root: ".mars".to_string(),
                    dest_path: "agents/coder.md".into(),
                    installed_checksum: "sha256:mars".into(),
                }],
            },
        );

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&lock, false),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 0);
        assert_eq!(
            std::fs::read_to_string(target.join("agents/coder.md")).unwrap(),
            "# hand-written"
        );
        let diagnostics = diag.drain();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "target-unmanaged-collision")
        );
    }

    #[test]
    fn sync_force_adopts_untracked_collision_in_linked_target() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# hand-written").unwrap();

        let mut lock = LockFile::empty();
        lock.items.insert(
            "agent/coder".to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs: vec![OutputRecord {
                    target_root: ".mars".to_string(),
                    dest_path: "agents/coder.md".into(),
                    installed_checksum: "sha256:mars".into(),
                }],
            },
        );

        let outcomes = vec![make_outcome("agents/coder.md", ActionTaken::Installed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".agents".to_string()],
            &outcomes,
            &target_sync_ctx(&lock, true),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        assert_eq!(
            std::fs::read_to_string(target.join("agents/coder.md")).unwrap(),
            "# Canonical"
        );
        assert!(!results[0].synced_outputs.is_empty());
        let diagnostics = diag.drain();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "target-unmanaged-adopted")
        );
    }

    fn make_skipped_skill_outcome(dest: &str, name: &str) -> ActionOutcome {
        ActionOutcome {
            item_id: crate::lock::ItemId {
                kind: ItemKind::Skill,
                name: ItemName::from(name),
            },
            action: ActionTaken::Skipped,
            dest_path: DestPath::from(dest),
            source_name: "test-source".into(),
            source_checksum: None,
            installed_checksum: None,
        }
    }

    fn make_installed_skill_outcome(dest: &str, name: &str) -> ActionOutcome {
        ActionOutcome {
            item_id: crate::lock::ItemId {
                kind: ItemKind::Skill,
                name: ItemName::from(name),
            },
            action: ActionTaken::Installed,
            dest_path: DestPath::from(dest),
            source_name: "test-source".into(),
            source_checksum: None,
            installed_checksum: None,
        }
    }

    #[test]
    fn sync_skipped_native_skill_projection_skips_byte_identical_rewrite() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".claude");
        let skill_source = mars_dir.join("skills/planning");
        std::fs::create_dir_all(skill_source.join("variants/claude")).unwrap();
        std::fs::write(
            skill_source.join("SKILL.md"),
            "---\nname: planning\ndescription: Base\n---\n# Base\n",
        )
        .unwrap();
        std::fs::write(skill_source.join("variants/claude/SKILL.md"), "# Claude").unwrap();

        let outcomes = vec![make_installed_skill_outcome("skills/planning", "planning")];
        let mut diag = DiagnosticCollector::new();
        sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        let native_skill = target.join("skills/planning/SKILL.md");
        assert!(native_skill.exists());
        let expected = std::fs::read_to_string(&native_skill).unwrap();
        let before = std::fs::metadata(&native_skill)
            .unwrap()
            .modified()
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(1100));

        let skill_dir = target.join("skills/planning");
        let checksum = hash::compute_hash(&skill_dir, ItemKind::Skill).unwrap();
        let lock =
            lock_with_skill_target_outputs(".claude", &[("skills/planning", checksum.as_str())]);
        let outcomes2 = vec![make_skipped_skill_outcome("skills/planning", "planning")];
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &outcomes2,
            &target_sync_ctx(&lock, false),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 0);
        let after = std::fs::metadata(&native_skill)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            before, after,
            "no-op sync must not rewrite native skill output"
        );
        assert_eq!(std::fs::read_to_string(&native_skill).unwrap(), expected);
    }

    #[test]
    fn sync_changed_native_skill_projection_rewrites_target_output() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".claude");
        let skill_source = mars_dir.join("skills/planning");
        std::fs::create_dir_all(skill_source.join("variants/claude")).unwrap();
        std::fs::write(
            skill_source.join("SKILL.md"),
            "---\nname: planning\ndescription: Base\n---\n# Base\n",
        )
        .unwrap();
        std::fs::write(skill_source.join("variants/claude/SKILL.md"), "# Claude v1").unwrap();

        let outcomes = vec![make_installed_skill_outcome("skills/planning", "planning")];
        let mut diag = DiagnosticCollector::new();
        sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &outcomes,
            &target_sync_ctx(&LockFile::empty(), false),
            &mut diag,
        );

        let expected =
            std::fs::read_to_string(target.join("skills/planning/SKILL.md")).unwrap();

        std::fs::write(skill_source.join("variants/claude/SKILL.md"), "# Claude v2").unwrap();
        let outcomes2 = vec![make_installed_skill_outcome("skills/planning", "planning")];
        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".claude".to_string()],
            &outcomes2,
            &target_sync_ctx(
                &lock_with_skill_target_outputs(".claude", &[("skills/planning", "sha256:old")]),
                false,
            ),
            &mut diag,
        );

        assert_eq!(results[0].items_synced, 1);
        let updated = std::fs::read_to_string(target.join("skills/planning/SKILL.md")).unwrap();
        assert!(updated.contains("# Claude v2"));
        assert_ne!(updated, expected);
    }
}
