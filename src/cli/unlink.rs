//! `mars unlink <target>` — remove Mars-owned content from a managed target.

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
    let target_name = crate::config::migrations::link::normalize_link(&parsed_target).target;

    let mars_dir = ctx.project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    let mut config = crate::config::load(&ctx.project_root)?;
    let mut lock = crate::lock::load(&ctx.project_root)?;
    let mut settings_updated = false;
    let mut target_was_managed = false;

    if config
        .settings
        .managed_root
        .as_deref()
        .map(crate::config::migrations::link::normalize_link)
        .is_some_and(|link| link.target == target_name)
    {
        config.settings.managed_root = None;
        settings_updated = true;
        target_was_managed = true;
    }

    if let Some(targets) = config.settings.targets.as_mut() {
        let old_len = targets.len();
        targets
            .retain(|t| crate::config::migrations::link::normalize_link(t).target != target_name);
        if targets.len() != old_len {
            settings_updated = true;
            target_was_managed = true;
        }
        if targets.is_empty() {
            config.settings.targets = None;
        }
    }

    let target_dir = ctx.project_root.join(&target_name);
    let (removed_outputs, diagnostics) = if target_was_managed {
        remove_owned_target_content(ctx, &target_name, &target_dir, &mut lock)?
    } else {
        (0, Vec::new())
    };
    let removed_dir = target_was_managed && remove_empty_ancestors(&target_dir, &target_dir)?;

    if settings_updated {
        crate::config::save(&ctx.project_root, &config)?;
    }
    if target_was_managed {
        crate::lock::write(&ctx.project_root, &lock)?;
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_name,
            "settings_updated": settings_updated,
            "removed_dir": removed_dir,
            "removed_outputs": removed_outputs,
            "diagnostics": diagnostics,
        }));
    } else if target_was_managed {
        output::print_success(&format!(
            "unlinked `{target_name}` (removed {removed_outputs} managed output{})",
            if removed_outputs == 1 { "" } else { "s" }
        ));
        output::print_diagnostics(&diagnostics);
    } else {
        output::print_info(&format!(
            "`{target_name}` is not a managed target; no changes made"
        ));
    }

    Ok(0)
}

fn remove_owned_target_content(
    ctx: &super::MarsContext,
    target_name: &str,
    target_dir: &std::path::Path,
    lock: &mut crate::lock::LockFile,
) -> Result<(usize, Vec<crate::diagnostic::Diagnostic>), MarsError> {
    let mut owned_paths: Vec<_> = lock
        .output_dest_paths_for_target(target_name)
        .into_iter()
        .collect();
    owned_paths.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));

    let mut removed = Vec::new();
    for dest_path in owned_paths {
        let path = target_dir.join(&dest_path);
        match remove_owned_path(&path) {
            Ok(()) => {
                removed.push((target_name.to_string(), dest_path));
                if let Some(parent) = path.parent() {
                    remove_empty_ancestors(parent, target_dir)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                removed.push((target_name.to_string(), dest_path));
            }
            Err(error) => return Err(error.into()),
        }
    }
    crate::lock::apply_removed_native_outputs(lock, &removed);

    let previous = lock
        .config_entries
        .get(target_name)
        .cloned()
        .map(|entries| std::collections::BTreeMap::from([(target_name.to_string(), entries)]))
        .unwrap_or_default();
    let removal_plan = crate::surface_ownership::retention::RemovalPlan::build(
        &previous,
        &std::collections::BTreeMap::new(),
    );
    let registry = crate::target::TargetRegistry::new();
    let mut diagnostics = crate::diagnostic::DiagnosticCollector::new();
    let retention = removal_plan.execute(
        |operation, diagnostics| {
            let surface = operation.surface();
            let Some(adapter) = registry.get(operation.target_root()) else {
                let (target_dir, removal) = operation.into_parts(&ctx.project_root);
                return crate::surface_ownership::retention::RemovalReport::failed(
                    format!("no adapter registered for `{}`", target_dir.display()),
                    removal.prior_records.clone(),
                );
            };
            match surface {
                crate::surface_ownership::retention::Surface::Hook => {
                    adapter.remove_owned_hook_entries(operation, &ctx.project_root, diagnostics)
                }
                crate::surface_ownership::retention::Surface::Mcp => {
                    adapter.remove_config_entries(operation, &ctx.project_root)
                }
            }
        },
        &mut diagnostics,
    );
    lock.config_entries.remove(target_name);
    lock.config_entries
        .extend(retention.into_retained_records());

    Ok((removed.len(), diagnostics.drain()))
}

fn remove_owned_path(path: &std::path::Path) -> std::io::Result<()> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
}

fn remove_empty_ancestors(
    start: &std::path::Path,
    target_dir: &std::path::Path,
) -> std::io::Result<bool> {
    let mut current = Some(start);
    let mut removed_target = false;
    while let Some(path) = current {
        if !path.starts_with(target_dir) {
            break;
        }
        match std::fs::remove_dir(path) {
            Ok(()) => {
                removed_target |= path == target_dir;
                current = path.parent();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = path.parent();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotADirectory
                ) =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(removed_target)
}
