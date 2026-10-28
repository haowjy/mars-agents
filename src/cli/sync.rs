//! `mars sync` — resolve + install (make reality match config).

use crate::error::MarsError;
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};

use super::output;

/// Arguments for `mars sync`.
#[derive(Debug, clap::Args)]
pub struct SyncArgs {
    /// Overwrite local modifications for managed files.
    #[arg(long)]
    pub force: bool,

    /// Dry run — show what would change.
    #[arg(long)]
    pub diff: bool,

    /// Install exactly from lock file, error if stale.
    #[arg(long)]
    pub frozen: bool,

    /// Refresh models.dev catalog and harness probes synchronously before sync (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    pub refresh_models: bool,

    /// Use disk-only catalog/probe caches during sync; launch no refresh worker.
    #[arg(long, conflicts_with = "refresh_models")]
    pub no_refresh_models: bool,

    /// Suppress the post-sync upgrade hint line.
    #[arg(long)]
    pub no_upgrade_hint: bool,

    /// Show per-item detail for launch-time fields handled by meridian at spawn.
    #[arg(long)]
    pub verbose: bool,

    /// Ignore package `requires-mars` version constraints.
    #[arg(long)]
    pub ignore_requires_mars: bool,

    /// Ignore package `requires-meridian` version constraints.
    #[arg(long)]
    pub ignore_requires_meridian: bool,
}

/// Run `mars sync`.
pub fn run(args: &SyncArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let no_upgrade_hint = args.no_upgrade_hint || no_upgrade_hint_from_env();
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            force: args.force,
            dry_run: args.diff,
            frozen: args.frozen,
            refresh_models: args.refresh_models,
            no_refresh_models: args.no_refresh_models,
            check_upgrades: !no_upgrade_hint,
            ignore_requires_mars: args.ignore_requires_mars,
            ignore_requires_meridian: args.ignore_requires_meridian,
        },
        recovery: Default::default(),
        lossiness_mode: if args.verbose {
            crate::diagnostic::LossinessMode::Verbose
        } else {
            crate::diagnostic::LossinessMode::Surface
        },
    };

    let report = crate::sync::execute(ctx, &request)?;

    output::print_sync_report(&report, json, no_upgrade_hint);

    Ok(0)
}

fn no_upgrade_hint_from_env() -> bool {
    match std::env::var("MARS_NO_UPGRADE_HINT") {
        Ok(value) => value.trim() == "1",
        Err(_) => false,
    }
}
