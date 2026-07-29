//! `mars repair` — rebuild state from lock + dependencies.

use crate::error::MarsError;
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};

use super::output;

/// Arguments for `mars repair`.
#[derive(Debug, clap::Args)]
pub struct RepairArgs {
    /// Ignore package `requires-mars` version constraints.
    #[arg(long)]
    pub ignore_requires_mars: bool,

    /// Ignore package `requires-meridian` version constraints.
    #[arg(long)]
    pub ignore_requires_meridian: bool,
}

/// Run `mars repair`.
///
/// Re-syncs everything from config. This is effectively a forced sync
/// that rebuilds the state. If lock exists, items are re-installed from
/// dependencies to match it. If lock is missing, a fresh sync is performed.
pub fn run(args: &RepairArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    if !json {
        output::print_info("repairing — re-syncing from dependencies...");
    }

    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            force: true,
            ignore_requires_mars: args.ignore_requires_mars,
            ignore_requires_meridian: args.ignore_requires_meridian,
            ..SyncOptions::default()
        },
        recovery: crate::sync::RecoveryPolicy::Repair,
        lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
    };

    // Force sync: overwrites everything, rebuilds from dependencies.
    let report = crate::sync::execute(ctx, &request)?;

    output::print_sync_report(&report, json, true);

    Ok(if report.recovery_halt.is_some() { 2 } else { 0 })
}
