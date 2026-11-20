//! `mars link <target>` — add a managed target directory.
//!
//! `mars link <target>` adds the target to `settings.targets` and syncs, so the target gets what
//! a sync installs. With `--local` it adds the target to `mars.local.toml` instead, so only this
//! machine syncs it.
//! Use `mars unlink <target>` to remove a target.

use crate::error::MarsError;
use crate::sync::{ConfigMutation, ResolutionMode, SyncOptions, SyncRequest};
use crate::types::managed_cmd;

use super::output;

/// Arguments for `mars link`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Target directory to materialize (e.g. `.claude`).
    pub target: String,
    /// Same as `mars sync --force`: adopt colliding paths in every target and reset local edits
    /// to managed items in `.mars/`.
    #[arg(long)]
    pub force: bool,
    /// Add the target to `mars.local.toml`: this machine syncs it, and `mars.toml` and the
    /// committed `mars.lock` do not change.
    #[arg(long)]
    pub local: bool,
}

/// Run `mars link`.
pub fn run(args: &LinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let parsed_target = super::target::normalize_target_name(&args.target)?;
    if let Some(retired) = mars_schema::retired::retired_target(&parsed_target) {
        return Err(MarsError::Link {
            target: retired.dir.to_string(),
            message: retired.message(&crate::types::managed_cmd(&format!(
                "mars unlink {}",
                retired.dir
            ))),
        });
    }
    let target_name = crate::config::migrations::link::normalize_link(&parsed_target).target;
    link_target(ctx, &target_name, args.force, args.local, json)
}

fn link_target(
    ctx: &super::MarsContext,
    target_name: &str,
    force: bool,
    local_only: bool,
    json: bool,
) -> Result<i32, MarsError> {
    let config_path = ctx.project_root.join("mars.toml");
    if !config_path.exists() {
        return Err(MarsError::Link {
            target: target_name.to_string(),
            message: format!(
                "mars.toml not found at {} — run `{cmd}` first",
                ctx.project_root.display(),
                cmd = managed_cmd("mars init"),
            ),
        });
    }

    if !json && !super::is_recognized_target(target_name) {
        output::warn(
            "target-unrecognized",
            format!("`{target_name}` is not a recognized tool directory — managing anyway"),
        );
    }

    let config = crate::config::load(&ctx.project_root)?;
    let local = crate::config::load_local(&ctx.project_root)?;
    let (settings_changed, local_changed) =
        crate::sync::mutation::link_changes(&config, &local, target_name, local_only);
    let in_project = crate::config::targets::project_targets(&config).contains(target_name);

    // A sync that names the target installs everything into it: files, native agents, hooks and
    // MCP servers.
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: Some(ConfigMutation::LinkTarget {
            target: target_name.to_string(),
            local: local_only,
        }),
        options: SyncOptions {
            force,
            ..SyncOptions::default()
        },
        recovery: Default::default(),
        lossiness_mode: crate::diagnostic::LossinessMode::Written,
    };
    let report = crate::sync::execute(ctx, &request)?;
    let (synced, removed) = report
        .target_outcomes
        .iter()
        .find(|outcome| outcome.target == target_name)
        .map_or((0, 0), |outcome| {
            (outcome.items_synced, outcome.items_removed)
        });

    if json {
        let mut value = output::sync_report_json(&report);
        if let Some(object) = value.as_object_mut() {
            object.extend([
                ("target".to_string(), target_name.into()),
                ("settings_updated".to_string(), (settings_changed || local_changed).into()),
                ("local".to_string(), local_only.into()),
                ("synced".to_string(), synced.into()),
                ("removed".to_string(), removed.into()),
            ]);
        }
        output::print_json(&value);
    } else {
        output::print_success(&format!(
            "managed target `{target_name}` (synced {synced}, removed {removed})"
        ));
        if local_only && in_project {
            output::print_info(&format!(
                "mars.toml already links `{target_name}`: it stays a project target, shared with everyone"
            ));
        }
        output::print_sync_report(&report, false, true, false);
    }

    Ok(0)
}
