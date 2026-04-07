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

    /// Skip the automatic models-cache refresh during sync.
    #[arg(long)]
    pub no_refresh_models: bool,
}

/// Run `mars sync`.
pub fn run(args: &SyncArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            force: args.force,
            dry_run: args.diff,
            frozen: args.frozen,
            no_refresh_models: args.no_refresh_models,
        },
    };

    let report = crate::sync::execute(ctx, &request)?;

    output::print_sync_report(&report, json);

    if report.has_conflicts() { Ok(1) } else { Ok(0) }
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parses_no_refresh_models() {
        let cli = Cli::try_parse_from(["mars", "sync", "--no-refresh-models"]).unwrap();
        let Command::Sync(args) = cli.command else {
            panic!("expected sync command");
        };
        assert!(args.no_refresh_models);
    }
}
