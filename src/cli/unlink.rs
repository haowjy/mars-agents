//! `mars unlink <target>` — remove Mars-owned content from a managed target.

use crate::error::MarsError;
use crate::target_sync::removal::{remove_empty_ancestors, remove_served, served_outputs};

use super::output;

/// Arguments for `mars unlink`.
#[derive(Debug, clap::Args)]
pub struct UnlinkArgs {
    /// Target directory to remove (e.g. `.agents`).
    pub target: String,
}

/// Run `mars unlink`.
pub fn run(args: &UnlinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let parsed_target = super::target::normalize_target_name(&args.target)?;
    let target_name = crate::config::migrations::link::normalize_link(&parsed_target).target;

    let mars_dir = ctx.project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    let mut config = crate::config::load(&ctx.project_root)?;
    let mut local = crate::config::load_local(&ctx.project_root)?;
    let mut lock = crate::sync::recovery::load_lock(&ctx.project_root)?;
    let mut settings_updated = false;
    let mut local_updated = false;
    let mut target_was_managed = false;

    if unset_target(&mut config.settings.targets, &target_name) {
        settings_updated = true;
        target_was_managed = true;
    }
    if unset_target(&mut local.settings.targets, &target_name) {
        local_updated = true;
        target_was_managed = true;
    }

    // A kill between link's lock write and its settings write leaves a claim in the lock
    // that no settings file lists; the lock is what `unlink` has to clean up.
    if super::target::lock_claims(&lock, &target_name) {
        target_was_managed = true;
    }

    let target_dir = ctx.project_root.join(&target_name);
    let had_dir = target_dir.exists();
    let (removed_outputs, diagnostics) = if target_was_managed {
        let (kept, served) = lock.split_targets(|target| target != target_name);
        lock = kept;
        let outputs = served_outputs(&served).len();
        (
            outputs,
            remove_served(&ctx.project_root, &mut lock, served)?,
        )
    } else {
        (0, Vec::new())
    };
    if target_was_managed {
        remove_empty_ancestors(&target_dir, &target_dir)?;
    }
    let removed_dir = target_was_managed && had_dir && !target_dir.exists();

    if settings_updated {
        crate::config::save(&ctx.project_root, &config)?;
    }
    if local_updated {
        crate::config::save_local(&ctx.project_root, &local)?;
    }
    if target_was_managed
        && let Some(warning) = crate::sync::recovery::publish(&ctx.project_root, &lock)?
    {
        output::warn("recovery-publish", warning);
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_name,
            "settings_updated": settings_updated || local_updated,
            "removed_dir": removed_dir,
            "removed_outputs": removed_outputs,
            "diagnostics": diagnostics,
        }));
    } else if target_was_managed {
        output::print_success(&format!(
            "unlinked `{target_name}` (removed {removed_outputs} managed output{})",
            if removed_outputs == 1 { "" } else { "s" }
        ));
        output::print_diagnostics(&diagnostics, false);
    } else {
        output::print_info(&format!(
            "`{target_name}` is not a managed target; no changes made"
        ));
    }

    Ok(0)
}

fn unset_target(targets: &mut Option<Vec<String>>, target_name: &str) -> bool {
    let Some(list) = targets.as_mut() else {
        return false;
    };
    let old_len = list.len();
    list.retain(|t| crate::config::migrations::link::normalize_link(t).target != target_name);
    let removed = list.len() != old_len;
    if list.is_empty() {
        *targets = None;
    }
    removed
}
