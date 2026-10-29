//! Phase 7: write the lock and build the `SyncReport`.

use std::path::Path;

use mars_schema::item::ItemKind;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::{SyncReport, SyncRequest, SyncedState, recovery};
use crate::types::MarsContext;

/// Phase 7: Write lock file, construct SyncReport.
///
/// Lock is written regardless of target sync outcome (D21).
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

    // Write lock file (D21 — regardless of target sync outcome).
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
        if let Some(warning) =
            crate::compiler::persist_lock_then_native_agent_manifest(project_root, &new_lock)?
        {
            diag.warn("native-agent-manifest-write", warning);
        }
        recovery::complete(project_root)?;

        // Best-effort models cache refresh: ensure the catalog covers any
        // new aliases we're about to persist. Sync never aborts on refresh
        // failure — warn and continue.
        let mars_path = ctx.project_root.join(".mars");
        let settings = &state
            .applied
            .planned
            .targeted
            .resolved
            .loaded
            .effective
            .settings;
        let ttl = settings.models_cache_ttl_hours();
        let providers = settings
            .catalog_providers
            .clone()
            .unwrap_or_else(crate::models::default_catalog_providers);
        let refresh = crate::models::resolve_models_refresh_control(
            request.options.refresh_models,
            request.options.no_refresh_models,
        )?;
        match crate::models::ensure_fresh_with_catalog_providers(
            &mars_path,
            ttl,
            refresh.catalog_mode,
            &providers,
        ) {
            Ok((_, outcome)) => {
                if let Some(warning) = crate::models::refresh_warning(&outcome) {
                    diag.warn("models-cache-refresh", warning);
                }
            }
            Err(err) => {
                diag.warn(
                    "models-cache-refresh",
                    format!("failed to refresh models cache: {err}"),
                );
            }
        }
    }

    for warning in &state.applied.planned.targeted.warnings {
        diag.warn(warning.code(), warning.message());
    }
    let dependency_changes = state
        .applied
        .planned
        .targeted
        .resolved
        .loaded
        .dependency_changes;
    let upgrades_available = state.applied.planned.targeted.resolved.upgrades_available;

    let diagnostics = diag.drain();

    Ok(SyncReport {
        applied: state.applied.applied,
        diagnostics,
        dependency_changes,
        upgrades_available,
        target_outcomes: state.target_outcomes,
        dry_run: request.options.dry_run,
        native_emitted,
        native_removed,
        hook_files_rewritten: state.rewritten_hook_files,
        recovery_halt: None,
        engine_fallbacks: diag.take_engine_fallbacks(),
    })
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
                || (matches!(item.kind, ItemKind::Hook)
                    && !output.dest_path.as_str().starts_with("hooks/"))
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
