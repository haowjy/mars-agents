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
    let mut local = crate::config::load_local(&ctx.project_root)?;
    let mut lock = crate::sync::recovery::load_lock(&ctx.project_root)?;
    let mut settings_updated = false;
    let mut local_updated = false;
    let mut target_was_managed = false;

    if unset_managed_root(&mut config.settings.managed_root, &target_name) {
        settings_updated = true;
        target_was_managed = true;
    }
    if unset_target(&mut config.settings.targets, &target_name) {
        settings_updated = true;
        target_was_managed = true;
    }
    if unset_managed_root(&mut local.settings.managed_root, &target_name) {
        local_updated = true;
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
    let (removed_outputs, diagnostics) = if target_was_managed {
        remove_owned_target_content(ctx, &target_name, &target_dir, &mut lock)?
    } else {
        (0, Vec::new())
    };
    let removed_dir = target_was_managed && remove_empty_ancestors(&target_dir, &target_dir)?;

    if settings_updated {
        crate::config::save(&ctx.project_root, &config)?;
    }
    if local_updated {
        crate::config::save_local(&ctx.project_root, &local)?;
    }
    if target_was_managed
        && let Some(warning) = crate::sync::recovery::publish(&ctx.project_root, &lock)?
    {
        output::print_warn(&warning);
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

fn unset_managed_root(managed_root: &mut Option<String>, target_name: &str) -> bool {
    let matches = managed_root
        .as_deref()
        .map(crate::config::migrations::link::normalize_link)
        .is_some_and(|link| link.target == target_name);
    if matches {
        *managed_root = None;
    }
    matches
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
        .unwrap_or_default();
    let registry = crate::target::TargetRegistry::new();
    let mut diagnostics = crate::diagnostic::DiagnosticCollector::new();
    let retention = crate::surface_ownership::retention::RemovalPlan::build(
        &std::collections::BTreeMap::from([(target_name.to_string(), previous.clone())]),
        &std::collections::BTreeMap::new(),
    )
    .execute(
        |operation, diagnostics| {
            crate::compiler::config_entries::remove_hooks(operation, &ctx.project_root, diagnostics)
        },
        &mut diagnostics,
    );
    let reconciled = crate::compiler::config_entries::reconcile_mcp(
        &registry,
        &ctx.project_root,
        target_name,
        Some(&previous),
        &[],
        false,
        false,
        &mut diagnostics,
    );
    crate::compiler::config_entries::report_removed(
        retention.removed_keys(),
        &std::collections::BTreeMap::from([(target_name.to_string(), reconciled.removed)]),
        &mut diagnostics,
    );
    lock.config_entries.remove(target_name);
    lock.config_entries
        .extend(retention.into_retained_records());
    if !reconciled.records.is_empty() {
        lock.config_entries
            .entry(target_name.to_string())
            .or_default()
            .extend(reconciled.records);
    }

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
