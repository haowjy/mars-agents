//! `mars remove <source>` — remove a source from config and prune its items.

use std::path::Path;

use crate::error::MarsError;

use super::output;

/// Arguments for `mars remove`.
#[derive(Debug, clap::Args)]
pub struct RemoveArgs {
    /// Name of the source to remove.
    pub source: String,
}

/// Run `mars remove`.
pub fn run(args: &RemoveArgs, root: &Path, json: bool) -> Result<i32, MarsError> {
    // Load config and remove source
    let mut config = crate::config::load(root)?;

    if !config.sources.contains_key(&args.source) {
        return Err(MarsError::Source {
            source_name: args.source.clone(),
            message: format!("source `{}` not found in agents.toml", args.source),
        });
    }

    config.sources.shift_remove(&args.source);

    // Run sync with proposed config; persist config only after validation passes.
    let report = super::sync::run_sync_with_config(root, &config, true, false, false, false)?;

    if !json {
        output::print_info(&format!("removed source `{}`", args.source));
    }

    output::print_sync_report(&report, json);

    if report.has_conflicts() { Ok(1) } else { Ok(0) }
}
