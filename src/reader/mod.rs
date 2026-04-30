/// Reader stage — source parsing, dependency resolution, local discovery.
///
/// `read()` is the first half of the sync pipeline. It acquires the sync lock,
/// loads config, resolves the dependency graph, and discovers local package
/// items. The result is a [`crate::model::ReaderIr`] that contains only
/// source-level facts — no dest paths, no rendered bytes, no lock records.
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::local_source;
use crate::model::ReaderIr;
use crate::sync::{SyncRequest, load_config, resolve_graph};
use crate::types::MarsContext;

/// Run the reader stage: lock → config → graph → local discovery → `ReaderIr`.
pub fn read(
    ctx: &MarsContext,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<ReaderIr, MarsError> {
    // Phase 1: acquire sync lock, load and mutate config.
    let loaded = load_config(ctx, request, diag)?;

    // Phase 2: resolve dependency graph + model aliases.
    let resolved = resolve_graph(ctx, loaded, request, diag)?;

    // Local package discovery — produces source paths only (no DestPath).
    // Dest-path assignment is the compiler's responsibility.
    let has_package = resolved.loaded.config.package.is_some();
    let local_source_name = crate::types::SourceOrigin::LocalPackage.to_string();
    let local_items = local_source::discover_local_items(
        &ctx.project_root,
        has_package,
        Some(local_source_name.as_str()),
        diag,
    )?;

    Ok(ReaderIr {
        resolved,
        local_items,
    })
}
