//! `mars unlink <target>` — remove a managed target directory.

use crate::error::MarsError;

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
    let target_name = crate::config::link_migration::normalize_link(&parsed_target).target;

    let mars_dir = ctx.project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    let mut config = crate::config::load(&ctx.project_root)?;
    let mut settings_updated = false;
    let mut target_was_managed = false;

    if config
        .settings
        .managed_root
        .as_deref()
        .map(crate::config::link_migration::normalize_link)
        .is_some_and(|link| link.target == target_name)
    {
        config.settings.managed_root = None;
        settings_updated = true;
        target_was_managed = true;
    }

    if let Some(targets) = config.settings.targets.as_mut() {
        let old_len = targets.len();
        targets.retain(|t| crate::config::link_migration::normalize_link(t).target != target_name);
        if targets.len() != old_len {
            settings_updated = true;
            target_was_managed = true;
        }
        if targets.is_empty() {
            config.settings.targets = None;
        }
    }

    // Delete directory before saving config so a failed deletion doesn't
    // leave settings mutated with the directory still on disk.
    let target_dir = ctx.project_root.join(&target_name);
    let removed_dir = if target_was_managed && target_dir.exists() {
        std::fs::remove_dir_all(&target_dir)?;
        true
    } else {
        false
    };

    if settings_updated {
        crate::config::save(&ctx.project_root, &config)?;
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_name,
            "settings_updated": settings_updated,
            "removed_dir": removed_dir,
        }));
    } else if removed_dir {
        output::print_success(&format!("removed managed target `{target_name}`"));
    } else if target_was_managed {
        output::print_info(&format!("removed `{target_name}` from settings"));
    } else {
        output::print_info(&format!(
            "`{target_name}` is not a managed target; no changes made"
        ));
    }

    Ok(0)
}
