//! Write the lock and build the `SyncReport`.

use std::path::Path;

use indexmap::IndexMap;
use mars_schema::item::ItemKind;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::LockFile;
use crate::sync::kept_hooks;
use crate::sync::{SyncReport, SyncRequest, SyncedState, recovery};
use crate::types::{MarsContext, SourceName};

pub(crate) fn finalize(
    ctx: &MarsContext,
    state: SyncedState,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<SyncReport, MarsError> {
    let project_root = &ctx.project_root;
    let old_lock = &state.applied.planned.targeted.resolved.loaded.old_lock;
    let graph = &state.applied.planned.targeted.resolved.graph;
    // Native-agent surface deltas for the summary: removals are unambiguous; emits
    // are filtered to new/changed outputs so steady-state re-emits stay quiet.
    let native_removed: Vec<(String, String)> = state.removed_native_outputs.clone();
    let native_emitted: Vec<(String, String)> = state
        .compiled_native_outputs
        .iter()
        .filter(|out| crate::lock::native_output_is_new_or_changed(old_lock, out))
        .map(|out| (out.target_root.clone(), out.dest_path.clone()))
        .collect();

    // Write lock file (regardless of target sync outcome).
    if !request.options.dry_run {
        let dep_models = crate::models::declaration_ordered_dep_models(
            graph,
            &state.applied.planned.targeted.resolved.loaded.effective,
        );
        let mut dep_model_aliases = crate::models::dependency_alias_snapshot(&dep_models);
        dep_model_aliases.sort_keys();

        let mut new_lock = crate::lock::build(
            graph,
            &state.applied.applied,
            old_lock,
            state.config_entries,
        )?;
        new_lock.dependency_model_aliases = dep_model_aliases;
        record_hook_source_paths(&mut new_lock, &state.applied.planned.targeted.target, graph);
        let mut confirmed_output_removals: Vec<(String, String)> = state
            .target_outcomes
            .iter()
            .flat_map(|outcome| {
                outcome
                    .removed_dest_paths
                    .iter()
                    .map(|dest_path| (outcome.target.clone(), dest_path.clone()))
            })
            .collect();
        confirmed_output_removals.extend(state.removed_config_entry_outputs.iter().cloned());
        confirmed_output_removals.extend(state.removed_native_outputs.iter().cloned());
        crate::lock::apply_target_sync_outputs(&mut new_lock, &state.target_outcomes);
        crate::lock::apply_removed_native_outputs(
            &mut new_lock,
            &state.removed_config_entry_outputs,
        );
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.config_entry_outputs)?;
        crate::lock::apply_removed_native_outputs(&mut new_lock, &state.removed_native_outputs);
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.compiled_native_outputs)?;
        confirmed_output_removals.extend(retry_tombstone_removals(
            project_root,
            old_lock,
            &new_lock,
            diag,
        ));
        crate::lock::retain_unremoved_noncanonical_outputs(
            &mut new_lock,
            old_lock,
            &confirmed_output_removals,
        );
        new_lock.restore_unmanaged(
            &state
                .applied
                .planned
                .targeted
                .resolved
                .loaded
                .unmanaged_records,
        );
        new_lock.finish(unplaced_hook_records(
            &state.applied.planned.targeted,
            &crate::config::targets::managed_targets(
                &state
                    .applied
                    .planned
                    .targeted
                    .resolved
                    .loaded
                    .effective
                    .settings,
            ),
        )?);
        diag.settle(old_lock, &new_lock);
        if let Some(warning) = recovery::publish(project_root, &new_lock)? {
            diag.warn("native-agent-manifest-write", warning);
        }
    }

    let dependency_changes = state
        .applied
        .planned
        .targeted
        .resolved
        .loaded
        .dependency_changes;
    let upgrades = state.applied.planned.targeted.resolved.upgrades;
    // Only `mars upgrade` reports moves: any other command moves a dependency only because its
    // own change asked for it.
    let dependency_moves = if matches!(
        request.resolution,
        crate::sync::ResolutionMode::Maximize { .. }
    ) {
        crate::sync::upgrades::moved_dependencies(
            old_lock,
            graph,
            &state.applied.planned.targeted.resolved.loaded.effective,
        )
    } else {
        Vec::new()
    };

    let diagnostics = diag.drain();

    Ok(SyncReport {
        applied: state.applied.applied,
        diagnostics,
        dependency_changes,
        upgrades,
        dependency_moves,
        target_outcomes: state.target_outcomes,
        dry_run: request.options.dry_run,
        native_emitted,
        native_removed,
        hook_files_rewritten: state.rewritten_hook_files,
        config_files_changed: state.config_files_changed,
        recovery_halt: None,
        engine_fallbacks: diag.take_engine_fallbacks(),
    })
}

/// Record where each hook directory sits in its package, so a later sync finds the install
/// of a hook whose `hook.toml` stops reading (`kept_hooks::settle`). Every sync stamps them,
/// which also upgrades a lock that has none.
fn record_hook_source_paths(
    lock: &mut LockFile,
    target: &crate::sync::target::TargetState,
    graph: &crate::resolve::ResolvedGraph,
) {
    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Hook)
    {
        let Some(node) = graph.nodes.get(&item.source_name) else {
            continue;
        };
        let source_path =
            kept_hooks::lock_source_path(&node.rooted_ref.package_root, &item.source_path);
        if let Some(locked) = lock.items.get_mut(&crate::lock::item_key(&item.id))
            && locked.source == item.source_name
        {
            locked.source_path = source_path;
        }
    }
}

/// A hook on a target that takes none of it has no output to hang an item on. Its record has
/// none either, and `LockFile::finish` keeps it: the next sync tells a new or changed hook, or
/// a new target, from a loss it has already reported by this record.
fn unplaced_hook_records(
    targeted: &crate::sync::TargetedState,
    managed_targets: &[String],
) -> Result<IndexMap<String, crate::lock::LockedItemV2>, MarsError> {
    let graph = &targeted.resolved.graph;
    targeted
        .target
        .unplaced_hook_losses(managed_targets)
        .map(|(hook, target)| {
            let source = SourceName::from(hook.source_name.as_str());
            let item = crate::lock::LockedItemV2 {
                source_path: graph.nodes.get(&source).and_then(|node| {
                    kept_hooks::lock_source_path(&node.rooted_ref.package_root, &hook.hook_dir)
                }),
                source,
                kind: ItemKind::Hook,
                version: None,
                source_checksum: crate::sync::target::hook_checksum(hook)?,
                outputs: Vec::new(),
            };
            Ok((crate::lock::unplaced_hook_key(&hook.def.name, target), item))
        })
        .collect()
}

fn retry_tombstone_removals(
    project_root: &Path,
    old_lock: &crate::lock::LockFile,
    current_lock: &crate::lock::LockFile,
    diag: &mut DiagnosticCollector,
) -> Vec<(String, String)> {
    let mut removed = Vec::new();
    for item in old_lock.items.values().filter(|item| {
        !item
            .outputs
            .iter()
            .any(|output| output.target_root == crate::lock::CANONICAL_TARGET_ROOT)
    }) {
        for output in &item.outputs {
            let current_pass_owns_output = current_lock.items.values().any(|current_item| {
                current_item.outputs.iter().any(|current_output| {
                    current_output.target_root == crate::lock::CANONICAL_TARGET_ROOT
                }) && current_item.outputs.iter().any(|current_output| {
                    current_output.target_root == output.target_root
                        && crate::target::dest_paths_equivalent(
                            current_output.dest_path.as_str(),
                            output.dest_path.as_str(),
                        )
                })
            });
            if output.target_root == crate::lock::CANONICAL_TARGET_ROOT || current_pass_owns_output
            {
                continue;
            }

            let path = project_root
                .join(&output.target_root)
                .join(output.dest_path.as_str());
            let result = if matches!(item.kind, ItemKind::Agent)
                || crate::lock::is_generated_hook_file(item.kind, output.dest_path.as_str())
            {
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                }
            } else {
                crate::platform::fs::safe_remove(&path)
            };

            match result {
                Ok(()) => removed.push((
                    output.target_root.clone(),
                    output.dest_path.as_str().to_string(),
                )),
                Err(error) => diag.warn(
                    "tombstone-remove",
                    format!(
                        "could not remove tombstoned output `{}`: {error}",
                        path.display()
                    ),
                ),
            }
        }
    }
    removed
}
