//! `mars repair` — rebuild state from lock + dependencies.

use crate::error::{LockError, MarsError};
use crate::lock::LockFile;
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};

use super::output;

/// Arguments for `mars repair`.
#[derive(Debug, clap::Args)]
pub struct RepairArgs {}

/// Run `mars repair`.
///
/// Re-syncs everything from config. This is effectively a forced sync
/// that rebuilds the state. If lock exists, items are re-installed from
/// dependencies to match it. If lock is missing, a fresh sync is performed.
pub fn run(_args: &RepairArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    if !json {
        output::print_info("repairing — re-syncing from dependencies...");
    }

    match crate::lock::load(&ctx.project_root) {
        Ok(_) => {}
        Err(MarsError::Lock(LockError::Corrupt { message })) => {
            eprintln!("warning: {message}");
            eprintln!("warning: lock is corrupt, rebuilding from mars.toml + dependencies");
            crate::lock::write(&ctx.project_root, &LockFile::empty())?;
        }
        Err(err) => return Err(err),
    }

    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            force: true,
            ..SyncOptions::default()
        },
        lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
    };

    // Force sync: overwrites everything, rebuilds from dependencies.
    let report = crate::sync::execute(ctx, &request)?;

    output::print_sync_report(&report, json, true);

    Ok(0)
}
