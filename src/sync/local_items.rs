//! Project-local (`_self`) items joining the dependency target state.
//!
//! A consumer's `.mars-src/`, or a package repo's own items, never pass through the resolver:
//! they are folded into the target state here, after the dependency items, and win a destination
//! over a dependency item. A local item whose destination exists and is not Mars-owned is blocked
//! (agents, skills) or skipped (hooks) rather than overwritten.

use std::path::Path;

use mars_schema::item::ItemKind;

use super::ownership::Ownership;
use super::{LoadedConfig, ResolvedState, SyncRequest};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::hash;
use crate::local_source::{LocalDiscoveredItem, LocalItemContent};
use crate::lock::ItemId;
use crate::sync::target::{self, RenamableKind, TargetItem, TargetState, default_dest_path};
use crate::types::{ContentHash, MarsContext, SourceName, SourceOrigin, managed_cmd};

/// Fold every discovered local item into `target_state`.
pub(super) fn merge_local_items(
    ctx: &MarsContext,
    resolved: &ResolvedState,
    local_items: Vec<LocalDiscoveredItem>,
    request: &SyncRequest,
    target_state: &mut TargetState,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let merge = LocalMerge {
        ctx,
        resolved,
        request,
        local_source_name: SourceOrigin::LocalPackage.to_string().into(),
        owns: resolved.loaded.ownership(),
        // `.mars/` is the canonical content root for diff and collision checks.
        managed_root: ctx.project_root.join(".mars"),
    };
    let managed_targets = resolved.loaded.effective.settings.managed_targets();
    let mcp_targets = target::mcp_config_targets(&managed_targets);
    for item in local_items {
        match &item.content {
            LocalItemContent::Mcp(mcp) => {
                target_state.select_mcp(mcp.as_ref().clone(), &mcp_targets);
            }
            LocalItemContent::Hook(hook) => {
                merge.hook(hook, &managed_targets, target_state, diag)?;
            }
            _ => merge.item(item, target_state, diag)?,
        }
    }
    Ok(())
}

struct LocalMerge<'a> {
    ctx: &'a MarsContext,
    resolved: &'a ResolvedState,
    request: &'a SyncRequest,
    local_source_name: SourceName,
    owns: Ownership<'a>,
    managed_root: std::path::PathBuf,
}

impl LocalMerge<'_> {
    fn loaded(&self) -> &LoadedConfig {
        &self.resolved.loaded
    }

    fn hook(
        &self,
        hook: &crate::compiler::hooks::ParsedHookItem,
        managed_targets: &[String],
        target_state: &mut TargetState,
        diag: &mut DiagnosticCollector,
    ) -> Result<(), MarsError> {
        for (target_name, dest_path, target_item) in
            target::hook_target_items(hook, &self.local_source_name, managed_targets)?
        {
            if let Some(existing) = target_state.items.shift_remove(&dest_path)
                && existing.source_hash != target_item.source_hash
            {
                diag.warn(
                    "local-shadow",
                    format!(
                        "local hook `{}` shadows dependency `{}` hook on target `{target_name}`",
                        hook.def.name, existing.source_name
                    ),
                );
            }
            target_state.select_hook(&target_name, &target_item, hook.clone());
            let disk_path = dest_path.resolve(&self.managed_root);
            if !self.owns.owns(&dest_path) && disk_path.symlink_metadata().is_ok() {
                diag.warn(
                    "unmanaged-collision",
                    format!(
                        "local hook `{}` collides with unmanaged path `{dest_path}` — leaving existing content untouched",
                        hook.def.name
                    ),
                );
                continue;
            }
            target_state.items.insert(dest_path, target_item);
        }
        Ok(())
    }

    /// A local agent or skill.
    fn item(
        &self,
        item: LocalDiscoveredItem,
        target_state: &mut TargetState,
        diag: &mut DiagnosticCollector,
    ) -> Result<(), MarsError> {
        let kind = item.discovered.id.kind;
        let is_flat_skill =
            kind == ItemKind::Skill && item.discovered.source_path == Path::new(".");
        let excluded_paths =
            if is_flat_skill {
                let mut targets = self.loaded().effective.settings.managed_targets();
                targets.extend(
                    self.loaded().old_lock.items.values().flat_map(|item| {
                        item.outputs.iter().map(|output| output.target_root.clone())
                    }),
                );
                targets.sort();
                targets.dedup();
                crate::local_source::flat_skill_excluded_paths(
                    &self.ctx.project_root,
                    &item.disk_path(),
                    &targets,
                )?
            } else {
                Vec::new()
            };
        let item_key = format!("{}:{}", kind, item.discovered.id.name);
        let source_path = crate::staging::materialize_local_prepared(
            &item,
            &self.loaded().effective.skills,
            self.resolved.preparation.path(),
            &item_key,
            &excluded_paths,
        )?;
        let source_hash = if is_flat_skill {
            ContentHash::from(hash::compute_skill_hash_filtered(
                &source_path,
                crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
            )?)
        } else {
            ContentHash::from(hash::compute_hash(&source_path, kind)?)
        };
        if kind == ItemKind::Agent
            && let Err(message) =
                crate::target::validate_agent_filename(item.discovered.id.name.as_str())
        {
            diag.error_with_category(
                "invalid-agent-filename",
                format!("{message}; skipping local agent"),
                crate::diagnostic::DiagnosticCategory::Validation,
            );
            return Ok(());
        }
        let dest_path = default_dest_path(
            RenamableKind::of(kind).expect("local items are agents, skills or bootstrap docs"),
            item.discovered.id.name.as_str(),
        );

        if let Some(existing) = target_state.items.shift_remove(&dest_path)
            && existing.source_hash != source_hash
        {
            diag.warn(
                "local-shadow",
                format!(
                    "local {} `{}` shadows dependency `{}` {} `{}`",
                    kind,
                    item.discovered.id.name,
                    existing.source_name,
                    existing.id.kind,
                    existing.id.name
                ),
            );
        }

        let disk_path = dest_path.resolve(&self.managed_root);
        let unowned_collision = !self.owns.owns(&dest_path) && disk_path.symlink_metadata().is_ok();
        let adoptable = unowned_collision
            && force_adoptable_self_destination(&disk_path, &self.managed_root, kind);
        if unowned_collision && !(self.request.options.force && adoptable) {
            let guidance = if adoptable {
                format!(
                    "use `{}` to replace and adopt this regular destination",
                    managed_cmd("mars sync --force")
                )
            } else {
                "relocate the destination and retry sync (force cannot adopt symlinks or wrong-shaped paths)".to_string()
            };
            return Err(MarsError::Source {
                source_name: self.local_source_name.to_string(),
                message: format!(
                    "selected self {} `{}` from `{}` is blocked by unmanaged destination `{}`; {guidance}",
                    kind,
                    item.discovered.id.name,
                    item.disk_path().display(),
                    disk_path.display(),
                ),
            });
        }
        if self.request.options.force && adoptable {
            let action = if self.request.options.dry_run {
                "would replace and adopt"
            } else {
                "will replace and adopt"
            };
            diag.warn(
                "self-adopt",
                format!(
                    "selected self {} `{}` {action} existing canonical destination `{}`",
                    kind,
                    item.discovered.id.name,
                    disk_path.display()
                ),
            );
        }

        target_state.items.insert(
            dest_path.clone(),
            TargetItem {
                id: ItemId {
                    kind,
                    name: item.discovered.id.name.clone(),
                },
                source_name: self.local_source_name.clone(),
                source_path,
                dest_path,
                source_hash,
                is_flat_skill,
                rewritten_content: None,
            },
        );
        Ok(())
    }
}

fn force_adoptable_self_destination(path: &Path, managed_root: &Path, kind: ItemKind) -> bool {
    let Some(relative) = path.strip_prefix(managed_root).ok() else {
        return false;
    };
    let Ok(root_metadata) = managed_root.symlink_metadata() else {
        return false;
    };
    if root_metadata.file_type().is_symlink() {
        return false;
    }
    let mut current = managed_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let Ok(metadata) = current.symlink_metadata() else {
            break;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
    }
    match path.symlink_metadata() {
        Ok(metadata) => match kind {
            ItemKind::Agent => metadata.is_file(),
            ItemKind::Skill => metadata.is_dir(),
            _ => false,
        },
        Err(_) => false,
    }
}
