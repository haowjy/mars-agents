/// Compiler stage — target building, diff, plan, apply, lock finalization.
///
/// `compile()` is the second half of the sync pipeline. It consumes a
/// [`crate::model::ReaderIr`] (all source-level facts) and produces a
/// [`crate::sync::SyncReport`] by assigning dest paths, computing diffs,
/// writing files, syncing managed targets, and persisting the lock.
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::model::ReaderIr;
use crate::sync::{
    LoadedConfig, ResolvedState, SyncReport, SyncRequest, apply_plan, build_target,
    check_frozen_gate, create_plan, finalize, sync_targets,
};
use crate::types::MarsContext;

/// Run the compiler stage: `ReaderIr` → target state → plan → apply → `SyncReport`.
pub fn compile(
    ctx: &MarsContext,
    ir: ReaderIr,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<SyncReport, MarsError> {
    // Reconstruct the phase struct the compiler phases expect.
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config: ir.raw_config,
            local: ir.local_config,
            effective: ir.config,
            old_lock: ir.old_lock,
            dependency_changes: ir.dependency_changes,
            _sync_lock: ir._sync_lock,
        },
        graph: ir.graph,
        model_aliases: ir.model_aliases,
    };

    // Phase 3: assign dest paths, handle collisions, rewrite frontmatter refs.
    let targeted = build_target(ctx, resolved, ir.local_items, request, diag)?;

    // Phase 4: diff + plan.
    let planned = create_plan(ctx, targeted, request, diag)?;

    // Frozen gate: no pending changes allowed.
    if request.options.frozen {
        check_frozen_gate(&planned)?;
    }

    // Phase 5: persist config mutations, apply plan to canonical store.
    let applied = apply_plan(ctx, planned, request)?;

    // Phase 6: copy from canonical store to managed target directories.
    let synced = sync_targets(ctx, applied, request, diag);

    // Phase 7: write lock file, build report.
    finalize(ctx, synced, request, diag)
}
