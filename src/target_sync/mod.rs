//! Target sync — copy content from .mars/ canonical store to managed targets.
//!
//! After `apply_plan()` writes resolved content to `.mars/agents/` and `.mars/skills/`,
//! this module copies that content to all configured native target directories (`.claude/`, etc.).
//!
//! All targets are managed outputs — they get copies (not symlinks) of .mars/ content.

#[cfg(test)]
use mars_schema::item::ItemKind;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::{LockFile, RecordRef};
use crate::platform::fs as fs_ops;
use crate::surface_ownership;
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::ContentHash;

pub(crate) mod removal;

#[derive(Debug, Clone)]
pub struct TargetSyncedOutput {
    pub dest_path: String,
    pub installed_checksum: ContentHash,
}

#[derive(Debug, Clone)]
pub struct TargetSyncOutcome {
    pub target: String,
    pub items_synced: usize,
    pub items_removed: usize,
    pub errors: Vec<String>,
    pub synced_outputs: Vec<TargetSyncedOutput>,
    pub removed_dest_paths: Vec<String>,
}

pub struct TargetSyncContext<'a> {
    pub old_lock: &'a LockFile,
    pub force: bool,
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

/// Where `outcome` lands inside `target_name`; `None` when it belongs to another target.
pub(crate) fn target_dest_rel(outcome: &ActionOutcome, target_name: &str) -> Option<String> {
    if crate::provided::is_provided_source(outcome.source_name.as_ref())
        && !crate::harness::registry::takes_provided(target_name)
    {
        return None;
    }
    match crate::sync::package_dir::PackageDir::parse_canonical(
        outcome.item_id.kind,
        &outcome.dest_path,
    ) {
        Some(dir) => dir.belongs_to(target_name).then(|| dir.native()),
        None => Some(outcome.dest_path.to_string()),
    }
}

/// Whether `dest` holds something other than what the lock records Mars writing there. A native
/// projection that differs from what this Mars would render, with the recorded bytes still on
/// disk, is Mars's own format change and an ordinary update; one that differs from the record was
/// edited.
fn edited_since_written(
    dest: &Path,
    kind: mars_schema::item::ItemKind,
    target_name: &str,
    dest_rel: &str,
    old_lock: &LockFile,
) -> bool {
    old_lock
        .installed_checksum_for_output(target_name, dest_rel)
        .is_some_and(|recorded| {
            crate::hash::compute_hash(dest, kind)
                .map(ContentHash::from)
                .ok()
                .as_ref()
                != Some(recorded)
        })
}

/// What a dry run shows of `sync_one_target`'s skill refresh: a native skill projection that
/// differs from what a real sync would write is repaired, so it is named here as it is there.
pub fn preview_native_skill_repairs(
    project_root: &Path,
    mars_dir: &Path,
    old_lock: &LockFile,
    targets: &[String],
    outcomes: &[ActionOutcome],
    diag: &mut DiagnosticCollector,
) {
    for target_name in targets {
        let Some(variant_key) =
            crate::target::spec(target_name).and_then(|spec| spec.skill_variant_key())
        else {
            continue;
        };
        for outcome in outcomes {
            if outcome.item_id.kind != mars_schema::item::ItemKind::Skill
                || !matches!(outcome.action, ActionTaken::Skipped)
            {
                continue;
            }
            let Some(dest_rel) = target_dest_rel(outcome, target_name) else {
                continue;
            };
            let source = mars_dir.join(outcome.dest_path.as_str());
            let dest = project_root.join(target_name).join(&dest_rel);
            if !source.exists() || !surface_ownership::target_dest_exists(&dest) {
                continue;
            }
            let matches = crate::compiler::variants::prepare_native_skill_projection(
                &source,
                variant_key,
                &mut DiagnosticCollector::new(),
                outcome.item_id.name.as_str(),
            )
            .and_then(|projection| {
                crate::compiler::variants::prepared_native_skill_projection_matches(
                    &dest,
                    &projection,
                )
            })
            .unwrap_or(false);
            if !matches
                && edited_since_written(
                    &dest,
                    outcome.item_id.kind,
                    target_name,
                    &dest_rel,
                    old_lock,
                )
            {
                diag.warn(
                    "target-native-projection-repaired",
                    format!(
                        "diverged native projection would be repaired: {target_name}/{dest_rel}/SKILL.md"
                    ),
                );
            }
        }
    }
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
    let mut items_synced = 0;
    let mut items_removed = 0;
    let mut errors = Vec::new();
    let mut synced_outputs = Vec::new();
    let mut removed_dest_paths = Vec::new();

    std::fs::create_dir_all(target_root)?;

    let mut expected_paths: HashSet<String> = HashSet::new();
    let target_spec = crate::target::spec(target_name);
    let native_skill_variant_key = target_spec
        .and_then(|spec| spec.skill_variant_key())
        .map(str::to_owned);
    let target_accepts_canonical_agents = target_spec
        .map(|spec| {
            spec.default_dest_path(mars_schema::item::ItemKind::Agent, "__mars_probe__")
                .is_some()
        })
        .unwrap_or(true);

    for outcome in outcomes {
        if outcome.item_id.kind == mars_schema::item::ItemKind::BootstrapDoc {
            continue;
        }
        let canonical_dest_rel = outcome.dest_path.as_str();
        let Some(dest_rel) = target_dest_rel(outcome, target_name) else {
            continue;
        };
        let dest_rel = dest_rel.as_str();
        if outcome.item_id.kind == mars_schema::item::ItemKind::Agent
            && !target_accepts_canonical_agents
        {
            continue;
        }
        match &outcome.action {
            // The orphan pass below removes what a dropped item recorded.
            ActionTaken::Removed => {}
            ActionTaken::Skipped => {
                expected_paths.insert(dest_rel.to_string());
                let source = mars_dir.join(canonical_dest_rel);
                let dest = target_root.join(dest_rel);
                if source.exists() || source.symlink_metadata().is_ok() {
                    let should_refresh_native_skill = outcome.item_id.kind
                        == mars_schema::item::ItemKind::Skill
                        && native_skill_variant_key.is_some();
                    let dest_exists = surface_ownership::target_dest_exists(&dest);
                    // Hash the existing destination once: it decides whether the
                    // managed copy is current, becomes the recorded checksum when
                    // the copy is byte-identical, and tells a preserved copy apart
                    // from user edits. A native skill is compared through its
                    // projection instead.
                    let dest_hash = (dest_exists && !should_refresh_native_skill).then(|| {
                        crate::hash::compute_hash(&dest, outcome.item_id.kind)
                            .map(ContentHash::from)
                    });
                    let managed_dest_is_current_or_stale = dest_exists
                        && old_lock.contains_output(target_name, dest_rel)
                        && (should_refresh_native_skill
                            || dest_hash
                                .as_ref()
                                .and_then(|hash| hash.as_ref().ok())
                                .is_some_and(|actual| {
                                    dest_matches_recorded_or_desired(
                                        actual,
                                        target_name,
                                        dest_rel,
                                        old_lock,
                                        outcome.installed_checksum.as_ref(),
                                    )
                                }));
                    let identical = unclaimed_dest_is_desired(
                        &source,
                        &dest,
                        outcome,
                        target_name,
                        dest_rel,
                        native_skill_variant_key.as_deref(),
                        old_lock,
                    );
                    let wants_copy = force
                        || identical
                        || !dest_exists
                        || should_refresh_native_skill
                        || managed_dest_is_current_or_stale;
                    if wants_copy {
                        if should_copy_to_target(&dest, target_name, dest_rel, identical, ctx, diag)
                        {
                            if let Some(variant_key) = native_skill_variant_key.as_deref()
                                && should_refresh_native_skill
                            {
                                crate::compiler::variants::validate_skill_variants(
                                    &source,
                                    outcome.item_id.name.as_str(),
                                    diag,
                                );
                                let projection =
                                    match crate::compiler::variants::prepare_native_skill_projection(
                                        &source,
                                        variant_key,
                                        diag,
                                        outcome.item_id.name.as_str(),
                                    ) {
                                        Ok(projection) => projection,
                                        Err(e) => {
                                            errors
                                                .push(format!("failed to prepare {dest_rel}: {e}"));
                                            continue;
                                        }
                                    };
                                let target_projection_matches =
                                    crate::compiler::variants::prepared_native_skill_projection_matches(
                                        &dest,
                                        &projection,
                                    )
                                    .unwrap_or(false);
                                if target_projection_matches {
                                    projection.record_lossiness(
                                        outcome.item_id.name.as_str(),
                                        &[RecordRef::output(target_name, dest_rel)],
                                        diag,
                                    );
                                    synced_outputs.push(TargetSyncedOutput {
                                        dest_path: dest_rel.to_string(),
                                        installed_checksum: projection.installed_checksum,
                                    });
                                    continue;
                                }
                                let edited = edited_since_written(
                                    &dest,
                                    outcome.item_id.kind,
                                    target_name,
                                    dest_rel,
                                    old_lock,
                                );
                                match crate::compiler::variants::project_prepared_skill_for_target(
                                    &source,
                                    &dest,
                                    Some(&projection),
                                ) {
                                    Ok(wrote) => {
                                        if wrote {
                                            items_synced += 1;
                                        }
                                        projection.record_lossiness(
                                            outcome.item_id.name.as_str(),
                                            &[RecordRef::output(target_name, dest_rel)],
                                            diag,
                                        );
                                        synced_outputs.push(TargetSyncedOutput {
                                            dest_path: dest_rel.to_string(),
                                            installed_checksum: projection
                                                .installed_checksum
                                                .clone(),
                                        });
                                        if edited {
                                            diag.warn(
                                                "target-native-projection-repaired",
                                                format!(
                                                    "repaired diverged native projection: {target_name}/{dest_rel}/SKILL.md"
                                                ),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        errors.push(format!("failed to copy {dest_rel}: {e}"))
                                    }
                                }
                                continue;
                            }
                            match copy_item_to_target(
                                &source,
                                &dest,
                                outcome,
                                native_skill_variant_key.as_deref(),
                                &[RecordRef::output(target_name, dest_rel)],
                                diag,
                            ) {
                                Ok(true) => {
                                    items_synced += 1;
                                    record_synced_output(
                                        &mut synced_outputs,
                                        dest_rel,
                                        hash_target_dest(&dest, outcome.item_id.kind).ok(),
                                    );
                                }
                                Ok(false) => {
                                    // Byte-identical tracked content is still an installation
                                    // outcome even though no bytes moved; reuse the hash that
                                    // proved it.
                                    record_synced_output(
                                        &mut synced_outputs,
                                        dest_rel,
                                        dest_hash
                                            .as_ref()
                                            .and_then(|hash| hash.as_ref().ok())
                                            .cloned(),
                                    );
                                }
                                Err(e) => errors.push(format!("failed to copy {dest_rel}: {e}")),
                            }
                        }
                    } else if !should_refresh_native_skill && dest_exists {
                        // A skill this target projects a variant of is compared through
                        // its projection, not the canonical tree; every other item that
                        // kept local content is either unclaimed or user-edited, and the
                        // user needs to hear which.
                        if old_lock
                            .installed_checksum_for_output(target_name, dest_rel)
                            .is_none()
                        {
                            surface_ownership::warn_no_installed_claim_collision(
                                target_name,
                                dest_rel,
                                diag,
                            );
                        } else if let Some(expected_checksum) = &outcome.installed_checksum {
                            let hash =
                                dest_hash.expect("a present destination has a checksum result");
                            match hash {
                                Ok(actual) if &actual != expected_checksum => {
                                    surface_ownership::warn_diverged(target_name, dest_rel, diag);
                                }
                                Ok(_) => {}
                                Err(e) => errors
                                    .push(format!("failed to verify {dest_rel} checksum: {e}")),
                            }
                        }
                    }
                }
            }
            _ => {
                expected_paths.insert(dest_rel.to_string());
                let source = mars_dir.join(canonical_dest_rel);
                let dest = target_root.join(dest_rel);
                let source_exists = source.exists() || source.symlink_metadata().is_ok();
                let identical = source_exists
                    && unclaimed_dest_is_desired(
                        &source,
                        &dest,
                        outcome,
                        target_name,
                        dest_rel,
                        native_skill_variant_key.as_deref(),
                        old_lock,
                    );
                if source_exists
                    && should_copy_to_target(&dest, target_name, dest_rel, identical, ctx, diag)
                {
                    match copy_item_to_target(
                        &source,
                        &dest,
                        outcome,
                        native_skill_variant_key.as_deref(),
                        &[RecordRef::output(target_name, dest_rel)],
                        diag,
                    ) {
                        Ok(true) => {
                            items_synced += 1;
                            record_synced_output(
                                &mut synced_outputs,
                                dest_rel,
                                hash_target_dest(&dest, outcome.item_id.kind).ok(),
                            );
                        }
                        Ok(false) => {
                            // Existing desired bytes and newly copied bytes have the same
                            // ownership semantics.
                            record_synced_output(
                                &mut synced_outputs,
                                dest_rel,
                                hash_target_dest(&dest, outcome.item_id.kind).ok(),
                            );
                        }
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
        target_name,
        &expected_paths,
        old_lock,
        &mut removed_dest_paths,
        &mut errors,
        diag,
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
    identical: bool,
    ctx: &TargetSyncContext<'_>,
    diag: &mut DiagnosticCollector,
) -> bool {
    surface_ownership::resolve_copy(
        ctx.old_lock,
        target_name,
        dest_rel,
        surface_ownership::DestState::new(surface_ownership::target_dest_exists(dest), identical),
        ctx.force,
        diag,
    )
}

/// Whether `dest` has no installed claim and already holds what Mars would write there, as a
/// sync interrupted before its lock write leaves it. A claimed or absent `dest` is not asked.
fn unclaimed_dest_is_desired(
    source: &Path,
    dest: &Path,
    outcome: &ActionOutcome,
    target_name: &str,
    dest_rel: &str,
    native_skill_variant_key: Option<&str>,
    old_lock: &LockFile,
) -> bool {
    if !surface_ownership::target_dest_exists(dest)
        || old_lock
            .installed_checksum_for_output(target_name, dest_rel)
            .is_some()
    {
        return false;
    }
    let equal = match native_skill_variant_key {
        Some(key) if outcome.item_id.kind == mars_schema::item::ItemKind::Skill => {
            crate::compiler::variants::prepare_native_skill_projection(
                source,
                key,
                &mut DiagnosticCollector::new(),
                outcome.item_id.name.as_str(),
            )
            .and_then(|projection| {
                crate::compiler::variants::prepared_native_skill_projection_matches(
                    dest,
                    &projection,
                )
            })
        }
        _ if source.is_dir() => fs_ops::directory_trees_content_equal(source, dest),
        _ => fs_ops::file_content_equal(source, dest),
    };
    equal.unwrap_or(false)
}

fn dest_matches_recorded_or_desired(
    actual: &ContentHash,
    target_name: &str,
    dest_rel: &str,
    old_lock: &LockFile,
    desired_checksum: Option<&ContentHash>,
) -> bool {
    if desired_checksum.is_some_and(|desired| desired == actual) {
        return true;
    }
    old_lock.items.values().any(|item| {
        item.outputs.iter().any(|recorded| {
            recorded.target_root == target_name
                && crate::target::dest_paths_equivalent(recorded.dest_path.as_str(), dest_rel)
                && recorded.installed_checksum() == Some(actual)
        })
    })
}

fn hash_target_dest(
    dest: &Path,
    kind: mars_schema::item::ItemKind,
) -> Result<ContentHash, MarsError> {
    crate::hash::compute_hash(dest, kind).map(ContentHash::from)
}

fn record_synced_output(
    synced_outputs: &mut Vec<TargetSyncedOutput>,
    dest_rel: &str,
    installed_checksum: Option<ContentHash>,
) {
    if let Some(installed_checksum) = installed_checksum {
        synced_outputs.push(TargetSyncedOutput {
            dest_path: dest_rel.to_string(),
            installed_checksum,
        });
    }
}

fn copy_item_to_target(
    source: &Path,
    dest: &Path,
    outcome: &ActionOutcome,
    native_skill_variant_key: Option<&str>,
    concerns: &[RecordRef],
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    let kind = outcome.item_id.kind;
    if kind == mars_schema::item::ItemKind::Skill && native_skill_variant_key.is_some() {
        let item_name = outcome.item_id.name.as_str();
        crate::compiler::variants::validate_skill_variants(source, item_name, diag);
        return crate::compiler::variants::project_skill_for_target(
            source,
            dest,
            native_skill_variant_key,
            diag,
            item_name,
            concerns,
        );
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| MarsError::io("create copy parent", parent, error))?;
    }

    let metadata = std::fs::metadata(source)
        .map_err(|error| MarsError::io("read metadata for", source, error))?;

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

/// Remove each output the old lock records for `target_name` that this sync no longer expects.
fn cleanup_orphans(
    target_root: &Path,
    target_name: &str,
    expected: &HashSet<String>,
    old_lock: &LockFile,
    removed_dest_paths: &mut Vec<String>,
    errors: &mut Vec<String>,
    diag: &mut DiagnosticCollector,
) -> usize {
    let mut removed = 0;
    for (kind, record) in old_lock.output_records_for_target(target_name) {
        if !expected.contains(record.dest_path.as_str()) {
            removed += remove_recorded_output(
                target_root,
                target_name,
                kind,
                record,
                removed_dest_paths,
                errors,
                diag,
            );
        }
    }
    removed
}

/// Remove one output Mars recorded (see `removal::remove_recorded`) and say what became of its
/// record: an output that is gone or was kept loses it, a failed removal keeps it for the next
/// sync. Returns how many outputs were removed.
fn remove_recorded_output(
    target_root: &Path,
    target_name: &str,
    kind: mars_schema::item::ItemKind,
    record: &crate::lock::OutputRecord,
    removed_dest_paths: &mut Vec<String>,
    errors: &mut Vec<String>,
    diag: &mut DiagnosticCollector,
) -> usize {
    use removal::Removal;
    let dest_rel = record.dest_path.as_str();
    match removal::remove_recorded(&target_root.join(dest_rel), kind, record) {
        Removal::Absent => removed_dest_paths.push(dest_rel.to_string()),
        Removal::Removed => {
            removal::remove_emptied_parents(&target_root.join(dest_rel), dest_rel);
            removed_dest_paths.push(dest_rel.to_string());
            return 1;
        }
        Removal::Kept => {
            removal::report_kept(target_name, dest_rel, diag);
            removed_dest_paths.push(dest_rel.to_string());
        }
        Removal::Failed(error) => errors.push(format!("failed to remove {dest_rel}: {error}")),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticCollector;
    use crate::hash;
    use crate::lock::{LockFile, LockedItemV2, OutputRecord};
    use crate::sync::apply::{ActionOutcome, ActionTaken};
    use crate::types::DestPath;
    use mars_schema::names::ItemName;
    use tempfile::TempDir;

    fn make_outcome(dest: &str, action: ActionTaken) -> ActionOutcome {
        ActionOutcome {
            item_id: crate::lock::ItemId {
                kind: mars_schema::item::ItemKind::Agent,
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
                    source_path: None,
                    outputs: vec![OutputRecord::installed(
                        target.to_string(),
                        (*dest).into(),
                        (*checksum).into(),
                    )],
                },
            );
        }
        lock
    }

    fn target_sync_ctx<'a>(old_lock: &'a LockFile, force: bool) -> TargetSyncContext<'a> {
        TargetSyncContext {
            old_lock,
            force,
            orphan_preserve_paths: None,
        }
    }

    fn make_skipped_with_checksum(dest: &str, checksum: &str) -> ActionOutcome {
        let mut outcome = make_outcome(dest, ActionTaken::Skipped);
        outcome.installed_checksum = Some(checksum.into());
        outcome
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
    fn cleanup_orphans_removes_what_mars_wrote_and_keeps_what_changed() {
        let dir = TempDir::new().unwrap();
        let target_root = dir.path().join(".agents");
        std::fs::create_dir_all(target_root.join("agents")).unwrap();
        std::fs::write(target_root.join("agents/coder.md"), "# Managed").unwrap();
        std::fs::write(target_root.join("agents/orphan.md"), "# Orphan").unwrap();
        std::fs::write(target_root.join("agents/edited.md"), "# Edited by hand").unwrap();

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
                ("agents/orphan.md", &hash::hash_bytes(b"# Orphan")),
                ("agents/edited.md", &hash::hash_bytes(b"# As installed")),
            ],
        );
        let mut removed_dest_paths = Vec::new();
        let mut diag = DiagnosticCollector::new();
        let removed = cleanup_orphans(
            &target_root,
            ".agents",
            &expected,
            &previous,
            &mut removed_dest_paths,
            &mut Vec::new(),
            &mut diag,
        );

        assert_eq!(removed, 1);
        assert!(target_root.join("agents/coder.md").exists());
        assert!(!target_root.join("agents/orphan.md").exists());
        assert!(target_root.join("agents/edited.md").exists());
        removed_dest_paths.sort();
        assert_eq!(removed_dest_paths, ["agents/edited.md", "agents/orphan.md"]);
        let diagnostics = diag.drain();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "output-kept");
        assert!(diagnostics[0].message.contains("agents/edited.md"));
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
    fn sync_skipped_pending_deletion_reports_missing_installed_claim() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".agents");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/coder.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/coder.md"), "# User replacement").unwrap();

        let checksum = hash::hash_bytes(b"# Canonical");
        let outcomes = vec![make_skipped_with_checksum("agents/coder.md", &checksum)];
        let mut lock = lock_with_target_outputs(".agents", &[("agents/coder.md", "sha256:old")]);
        lock.items.get_mut("agent/coder.md").unwrap().outputs[0].mark_pending_deletion();
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
            "# User replacement"
        );
        let diagnostics = diag.drain();
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "target-unmanaged-collision"
                    && diagnostic.message.contains("no installed-content claim")
            }),
            "a pending-deletion record carries no installed-content authority"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != "target-divergent")
        );
    }

    #[test]
    fn sync_preserves_handwritten_collision_when_lock_only_tracks_mars() {
        let dir = TempDir::new().unwrap();
        let mars_dir = dir.path().join(".mars");
        let target = dir.path().join(".opencode");

        std::fs::create_dir_all(mars_dir.join("agents")).unwrap();
        std::fs::write(mars_dir.join("agents/design-lead.md"), "# Canonical").unwrap();
        std::fs::create_dir_all(target.join("agents")).unwrap();
        std::fs::write(target.join("agents/opencode-only-test.md"), "# custom").unwrap();
        std::fs::write(target.join("agents/design-lead.md"), "# hand-written").unwrap();

        let mut lock = LockFile::empty();
        lock.items.insert(
            "agent/design-lead".to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                source_path: None,
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "agents/design-lead.md".into(),
                    "sha256:mars".into(),
                )],
            },
        );

        let outcomes = vec![make_outcome("agents/design-lead.md", ActionTaken::Removed)];
        let mut diag = DiagnosticCollector::new();

        let results = sync_managed_targets(
            dir.path(),
            &mars_dir,
            &[".opencode".to_string()],
            &outcomes,
            &target_sync_ctx(&lock, false),
            &mut diag,
        );

        assert_eq!(results[0].items_removed, 0);
        assert!(target.join("agents/opencode-only-test.md").exists());
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
                source_path: None,
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "agents/coder.md".into(),
                    "sha256:mars".into(),
                )],
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
                source_path: None,
                outputs: vec![OutputRecord::installed(
                    ".mars".to_string(),
                    "agents/coder.md".into(),
                    "sha256:mars".into(),
                )],
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
}
