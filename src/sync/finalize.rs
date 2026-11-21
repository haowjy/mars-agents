//! Write the lock and build the `SyncReport`.

use std::collections::BTreeMap;
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
    let native_removed: Vec<(String, String)> = state.native_removals.deleted.clone();
    let native_emitted: Vec<(String, String)> = state
        .compiled_native_outputs
        .iter()
        .filter(|out| crate::lock::native_output_is_new_or_changed(old_lock, out))
        .map(|out| (out.target_root.clone(), out.dest_path.clone()))
        .collect();

    let dropped = &state.applied.planned.targeted.resolved.loaded.dropped;
    let mut dropped_targets: Vec<String> = dropped.recorded_targets().into_iter().collect();
    let mut pending_targets = Vec::new();
    // A dry run names what a real run would delete; a real run, what it did.
    let mut dropped_outputs = Vec::new();
    if request.options.dry_run {
        let preview = crate::target_sync::removal::preview_removal(project_root, dropped);
        dropped_outputs = preview.removed;
        if request.options.preview_warnings {
            for (root, dest_path) in &preview.kept {
                crate::target_sync::removal::report_kept(root, dest_path, diag);
            }
        }
    }

    // Write lock file (regardless of target sync outcome).
    if !request.options.dry_run {
        let dep_models = crate::models::declaration_ordered_dep_models(
            graph,
            &state.applied.planned.targeted.resolved.loaded.effective,
        );
        let mut dep_model_aliases = crate::models::dependency_alias_snapshot(&dep_models);
        dep_model_aliases.sort_keys();

        let provided_entries = provided_entries(
            &state.applied.planned.targeted.target,
            old_lock,
            &state.config_entries,
        );
        let mut new_lock = crate::lock::build(
            graph,
            &state.applied.applied,
            old_lock,
            state.config_entries,
            provided_entries,
            &state.applied.planned.targeted.resolved.loaded.provided,
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
        confirmed_output_removals.extend(state.native_removals.dropped_records().cloned());
        crate::lock::record_provided_skills(
            &mut new_lock,
            old_lock,
            &state.provided_skills,
            &state.target_outcomes,
        );
        crate::lock::apply_target_sync_outputs(&mut new_lock, &state.target_outcomes);
        crate::lock::apply_removed_native_outputs(
            &mut new_lock,
            &state.removed_config_entry_outputs,
        );
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.config_entry_outputs)?;
        crate::lock::apply_removed_native_outputs(
            &mut new_lock,
            state.native_removals.dropped_records(),
        );
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
        let loaded = &state.applied.planned.targeted.resolved.loaded;
        new_lock.join(loaded.carried.clone());
        let removed = crate::target_sync::removal::remove_served(
            project_root,
            &mut new_lock,
            loaded.dropped.clone(),
        )?;
        diag.extend(removed.diagnostics);
        dropped_outputs = removed.outputs;
        // A target whose removal failed keeps its records, and the next sync retries it.
        let remaining = new_lock.recorded_targets();
        (pending_targets, dropped_targets) = dropped_targets
            .into_iter()
            .partition(|target| remaining.contains(target));
        let project_targets = crate::config::targets::project_targets(&loaded.config);
        if let Some(warning) = recovery::publish(project_root, &new_lock, &project_targets)? {
            diag.warn("native-agent-manifest-write", warning);
        }
    }

    dropped_outputs.retain(|(root, _)| root != crate::lock::CANONICAL_TARGET_ROOT);

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
        dropped_targets,
        pending_targets,
        dropped_outputs,
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

/// The config entries a host's provided content wrote, which the lock saves in
/// `machine.lock`. A record carries no source, so this reads the selection: an entry is
/// the host's when a selected hook or MCP server of a provided source has its key. One no
/// selection names (a hook kept as the last sync installed it) stays where the last sync had it.
fn provided_entries(
    target: &crate::sync::target::TargetState,
    old_lock: &LockFile,
    records: &BTreeMap<String, BTreeMap<String, crate::lock::ConfigEntryRecord>>,
) -> std::collections::BTreeSet<(String, String)> {
    use crate::target::HookEntry;
    use crate::target::mcp_file::MCP_KIND;
    use crate::target::plugin::HOOK_ENTRY_KIND;

    let hooks = target.selected_hooks().map(|(dir, hook)| {
        let keys: Vec<String> = records
            .get(dir.target())
            .into_iter()
            .flat_map(|records| records.keys())
            .filter(|key| {
                **key == HOOK_ENTRY_KIND.key(&hook.def.name)
                    || HookEntry::parse_key(key).is_some_and(|(_, name)| name == hook.def.name)
            })
            .cloned()
            .collect();
        (dir.target(), hook.source_name.as_str(), keys)
    });
    let servers = target.selected_mcp().map(|(dir, server)| {
        (
            dir.target(),
            server.source_name.as_str(),
            vec![MCP_KIND.key(&server.name)],
        )
    });
    let mut provided = std::collections::BTreeSet::new();
    let mut claimed = std::collections::BTreeSet::new();
    for (target_root, source, keys) in hooks.chain(servers) {
        for key in keys {
            let owner = (target_root.to_string(), key);
            if crate::provided::is_provided_source(source) {
                provided.insert(owner);
            } else {
                claimed.insert(owner);
            }
        }
    }
    provided.extend(
        old_lock
            .provided_entries
            .iter()
            .filter(|entry| !claimed.contains(*entry))
            .cloned(),
    );
    provided.retain(|(target_root, key)| {
        records
            .get(target_root)
            .is_some_and(|records| records.contains_key(key))
    });
    provided
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
            // A tombstone is an output marked pending deletion; one with a checksum is installed.
            if output.installed_checksum().is_some() {
                continue;
            }
            let current_pass_owns_output = current_lock.items.values().any(|current_item| {
                current_item.outputs.iter().any(|current_output| {
                    current_output.installed_checksum().is_some()
                        && current_output.target_root == output.target_root
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
