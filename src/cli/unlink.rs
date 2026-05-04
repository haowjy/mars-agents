//! `mars unlink <target>` — remove a managed target directory.
//!
//! Thin wrapper around `link --unlink` for discoverability.

use crate::error::MarsError;

/// Arguments for `mars unlink`.
#[derive(Debug, clap::Args)]
pub struct UnlinkArgs {
    /// Target directory to remove (e.g. `.agents`).
    pub target: String,
}

/// Run `mars unlink`.
pub fn run(args: &UnlinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let link_args = super::link::LinkArgs {
        target: args.target.clone(),
        unlink: true,
    };
    super::link::run(&link_args, ctx, json)
}
