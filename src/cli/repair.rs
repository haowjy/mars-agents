//! `mars repair` — rebuild state from lock + sources.

use std::path::Path;

use crate::error::MarsError;
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};

use super::output;

/// Arguments for `mars repair`.
#[derive(Debug, clap::Args)]
pub struct RepairArgs {}

/// Run `mars repair`.
///
/// Re-syncs everything from config. This is effectively a forced sync
/// that rebuilds the state. If lock exists, items are re-installed from
/// sources to match it. If lock is missing, a fresh sync is performed.
pub fn run(_args: &RepairArgs, root: &Path, json: bool) -> Result<i32, MarsError> {
    if !json {
        output::print_info("repairing — re-syncing from sources...");
    }

    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            force: true,
            dry_run: false,
            frozen: false,
        },
    };

    // Force sync: overwrites everything, rebuilds from sources.
    let report = crate::sync::execute(root, &request)?;

    output::print_sync_report(&report, json);

    if report.has_conflicts() { Ok(1) } else { Ok(0) }
}
