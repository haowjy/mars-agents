use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::local_source;
use crate::model::ReaderIr;
use crate::sync::{SyncRequest, load_config, resolve_graph};
use crate::types::MarsContext;

pub fn read(
    ctx: &MarsContext,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
    enforce_preparation_gate: bool,
) -> Result<ReaderIr, MarsError> {
    let loaded = load_config(ctx, request, diag)?;

    let mut resolved = resolve_graph(ctx, loaded, request, diag)?;

    let package_name = resolved
        .loaded
        .config
        .package
        .as_ref()
        .map(|p| p.name.as_str());
    let local_items = local_source::discover_local_items(&ctx.project_root, package_name, diag)?;

    if enforce_preparation_gate {
        diag.fail_on_errors()?;
    }
    crate::staging::materialize_prepared_graph(
        &mut resolved.graph,
        &resolved.loaded.effective,
        resolved.preparation.path(),
        diag,
    )?;

    Ok(ReaderIr {
        resolved,
        local_items,
    })
}
