//! `mars link <dir>` — symlink agents/ and skills/ into another directory.
//!
//! Creates `<dir>/agents -> <mars-root>/agents` and `<dir>/skills -> <mars-root>/skills`.
//! Useful for tools that look in `.claude/`, `.cursor/`, etc. instead of `.agents/`.
//!
//! Persists the link in `agents.toml [settings] links` so `mars doctor` can verify it.

use std::path::Path;

use crate::error::MarsError;

use super::output;

/// Arguments for `mars link`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Target directory to create symlinks in (e.g. `.claude`).
    pub target: String,

    /// Remove symlinks instead of creating them.
    #[arg(long)]
    pub unlink: bool,
}

/// Run `mars link`.
pub fn run(args: &LinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let project_root = &ctx.project_root;
    let target_dir = if Path::new(&args.target).is_absolute() {
        std::path::PathBuf::from(&args.target)
    } else {
        project_root.join(&args.target)
    };

    if args.unlink {
        return unlink(ctx, &args.target, &target_dir, json);
    }

    // Create target directory if needed
    std::fs::create_dir_all(&target_dir).map_err(|e| MarsError::Source {
        source_name: "link".to_string(),
        message: format!("cannot create {}: {e}", target_dir.display()),
    })?;

    // Compute relative path from target dir back to mars root
    let rel_root = pathdiff::diff_paths(&ctx.managed_root, &target_dir).unwrap_or_else(|| ctx.managed_root.clone());

    let mut linked = 0;
    for subdir in ["agents", "skills"] {
        let link_path = target_dir.join(subdir);
        let link_target = rel_root.join(subdir);

        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            // Check if it's already the right symlink
            if link_path.symlink_metadata().is_ok()
                && link_path
                    .read_link()
                    .map(|t| t == link_target)
                    .unwrap_or(false)
            {
                if !json {
                    output::print_info(&format!("{}/{subdir} already linked", args.target));
                }
                continue;
            }

            return Err(MarsError::Source {
                source_name: "link".to_string(),
                message: format!(
                    "{} already exists — remove it first or use a different target",
                    link_path.display()
                ),
            });
        }

        // Ensure the source dir exists
        let source_dir = ctx.managed_root.join(subdir);
        if !source_dir.exists() {
            std::fs::create_dir_all(&source_dir)?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(&link_target, &link_path).map_err(|e| MarsError::Source {
            source_name: "link".to_string(),
            message: format!(
                "failed to create symlink {} -> {}: {e}",
                link_path.display(),
                link_target.display()
            ),
        })?;

        #[cfg(not(unix))]
        return Err(MarsError::Source {
            source_name: "link".to_string(),
            message: "symlinks are only supported on Unix".to_string(),
        });

        linked += 1;
    }

    // Persist the link in settings
    persist_link(&ctx.managed_root, &args.target)?;

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_dir.to_string_lossy(),
            "linked": linked,
        }));
    } else if linked > 0 {
        output::print_success(&format!(
            "linked agents/ and skills/ into {}",
            args.target
        ));
    } else {
        output::print_info(&format!("{} already fully linked", args.target));
    }

    Ok(0)
}

/// Remove symlinks created by `mars link`.
fn unlink(ctx: &super::MarsContext, target_name: &str, target_dir: &Path, json: bool) -> Result<i32, MarsError> {
    let mut removed = 0;

    for subdir in ["agents", "skills"] {
        let link_path = target_dir.join(subdir);

        // Only remove if it's a symlink (never delete real directories)
        if link_path.symlink_metadata().is_ok() && link_path.read_link().is_ok() {
            std::fs::remove_file(&link_path).map_err(|e| MarsError::Source {
                source_name: "link".to_string(),
                message: format!("failed to remove symlink {}: {e}", link_path.display()),
            })?;
            removed += 1;
        }
    }

    // Remove from settings
    remove_link(&ctx.managed_root, target_name)?;

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "removed": removed,
        }));
    } else if removed > 0 {
        output::print_success(&format!(
            "removed {} symlink(s) from {}",
            removed,
            target_dir.display()
        ));
    } else {
        output::print_info("no symlinks to remove");
    }

    Ok(0)
}

/// Add a link target to settings if not already present.
fn persist_link(root: &Path, target: &str) -> Result<(), MarsError> {
    let mut config = crate::config::load(root)?;
    if !config.settings.links.contains(&target.to_string()) {
        config.settings.links.push(target.to_string());
        crate::config::save(root, &config)?;
    }
    Ok(())
}

/// Remove a link target from settings.
fn remove_link(root: &Path, target: &str) -> Result<(), MarsError> {
    let mut config = crate::config::load(root)?;
    config.settings.links.retain(|l| l != target);
    crate::config::save(root, &config)?;
    Ok(())
}
