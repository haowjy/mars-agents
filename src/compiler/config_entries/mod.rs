//! Config-entry compiler lane for MCP servers and hooks.
//!
//! This module owns config-entry lowering and writes. MCP and hook selection
//! come from the target build that also selects installed content.

pub mod resolve;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::compiler::hooks::ParsedHookItem;
use crate::compiler::mcp::{ParsedMcpItem, TargetMcpEntry};
use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::sync::AppliedState;
use crate::target::{ConfigEntry, HookEntry, McpServerEntry, TargetAdapter};
use crate::types::MarsContext;

pub(crate) struct ConfigEntryCompilation {
    pub records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    pub emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub removed_outputs: Vec<(String, String)>,
}

pub(crate) fn file_hook_output_preserve_paths(
    lock: &crate::lock::LockFile,
) -> HashMap<String, std::collections::HashSet<String>> {
    let registry = crate::target::TargetRegistry::new();
    let mut preserve = HashMap::<String, std::collections::HashSet<String>>::new();
    for item in lock
        .items
        .values()
        .filter(|item| item.kind == crate::lock::ItemKind::Hook)
    {
        for output in item
            .outputs
            .iter()
            .filter(|output| output.target_root != crate::lock::CANONICAL_TARGET_ROOT)
        {
            let Some(adapter) = registry.get(&output.target_root) else {
                continue;
            };
            let owner_prefix = format!("hooks/{}/", output.target_root.trim_start_matches('.'));
            let is_file_fragment = item.outputs.iter().any(|canonical| {
                canonical.target_root == crate::lock::CANONICAL_TARGET_ROOT
                    && canonical
                        .dest_path
                        .as_str()
                        .strip_prefix(&owner_prefix)
                        .and_then(|name| adapter.hook_file_dest_path(name))
                        .is_some_and(|path| {
                            crate::target::dest_paths_equivalent(
                                path.to_string_lossy().as_ref(),
                                output.dest_path.as_str(),
                            )
                        })
            });
            if is_file_fragment {
                preserve
                    .entry(output.target_root.clone())
                    .or_default()
                    .insert(output.dest_path.as_str().to_string());
            }
        }
    }
    preserve
}

/// Validate all hook schemas and native event names before the apply phase.
///
/// This is deliberately separate from config emission so an invalid hook
/// cannot leave canonical or target state partially mutated.
pub(crate) fn preflight_config_entries(
    ctx: &MarsContext,
    targeted: &crate::sync::TargetedState,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> Result<(), crate::error::MarsError> {
    use crate::compiler::hooks::{load_file_fragment, load_merge_fragment};
    use crate::error::{ConfigError, MarsError};
    use crate::target::{HookFragmentMode, TargetRegistry};

    let resolved = &targeted.resolved;
    let registry = TargetRegistry::new();
    let mut errors = Vec::new();
    let mut touched_targets: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    for (target, records) in &resolved.loaded.old_lock.config_entries {
        let touch = touched_targets.entry(target.clone()).or_default();
        touch.0 |= records.keys().any(|key| key.starts_with("mcp:"));
        touch.1 |= records.keys().any(|key| key.starts_with("hook"));
    }
    for (target, _) in targeted.target.selected_hooks() {
        touched_targets.entry(target.to_string()).or_default().1 = true;
    }
    for target in targeted.target.selected_mcp_targets() {
        touched_targets.entry(target.to_string()).or_default().0 = true;
    }
    for (target_name, (touches_mcp, touches_hooks)) in &touched_targets {
        let target_dir = ctx.project_root.join(target_name);
        if let Err(error) = validate_destination_path(&ctx.project_root, &target_dir, true) {
            errors.push(error);
            continue;
        }
        if let Some(adapter) = registry.get(target_name) {
            let has_legacy_hooks = resolved
                .loaded
                .old_lock
                .config_entries
                .get(target_name)
                .is_some_and(|records| {
                    records.iter().any(|(key, record)| {
                        crate::surface_ownership::retention::Surface::of_key(key)
                            == crate::surface_ownership::retention::Surface::Hook
                            && record.emitted_json.is_none()
                    })
                });
            let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            if *touches_mcp {
                names.extend(adapter.mcp_config_file_names());
            }
            if *touches_hooks {
                names.extend(adapter.hook_config_file_names());
            }
            if has_legacy_hooks {
                names.extend(adapter.legacy_hook_config_file_names());
            }
            for name in names {
                let path = target_dir.join(name);
                if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                    errors.push(error);
                }
                if let Err(error) = crate::target::validate_json_config_file(&path) {
                    errors.push(error.to_string());
                }
            }
        }
    }
    for (target_name, item) in targeted.target.selected_hooks() {
        let adapter = registry.get(target_name);
        let mode = adapter.and_then(|adapter| adapter.hook_fragment_mode());
        let installed = installed_hook_dir(&ctx.project_root, target_name, &item.def.name);
        let hook_dir = portable_installed_hook_dir(&installed);
        if let Err(error) = validate_destination_path(&ctx.project_root, &installed, true) {
            errors.push(error);
            continue;
        }
        if !force
            && installed.symlink_metadata().is_ok()
            && !resolved
                .loaded
                .old_lock
                .contains_output(target_name, &format!("hooks/{}", item.def.name))
        {
            errors.push(format!(
                "unmanaged target hook directory `{}` blocks hook `{}`",
                installed.display(),
                item.def.name
            ));
            continue;
        }
        let result = match mode {
            Some(HookFragmentMode::MergeJson { events }) => {
                load_merge_fragment(item, target_name, events, &hook_dir, diag, true).map(|_| ())
            }
            Some(HookFragmentMode::File) => {
                if let Some(relative) =
                    adapter.and_then(|adapter| adapter.hook_file_dest_path(&item.def.name))
                {
                    let destination = ctx.project_root.join(target_name).join(relative);
                    if let Err(error) =
                        validate_destination_path(&ctx.project_root, &destination, false)
                    {
                        errors.push(error);
                        continue;
                    }
                }
                load_file_fragment(item, target_name, &hook_dir).map(|_| ())
            }
            None => Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "hook `{}`: target `{target_name}` has no hook fragment mechanism",
                    item.def.name
                ),
            })),
        };
        if let Err(error) = result {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(MarsError::Config(ConfigError::Invalid {
            message: errors.join("\n"),
        }))
    }
}

/// Lower the mergeable MCP and hook entries for one target.
///
/// The caller owns target-specific eligibility checks, including whether a
/// project-installed hook directory is current. `hook_dir` supplies the
/// replacement for `${MARS_HOOK_DIR}` in each hook's native fragment.
pub(crate) fn lower_config_entries_for_target(
    hooks: &[&ParsedHookItem],
    mcp: &[&ParsedMcpItem],
    adapter: &dyn TargetAdapter,
    hook_dir: impl Fn(&ParsedHookItem) -> String,
    diag: &mut DiagnosticCollector,
) -> Vec<ConfigEntry> {
    use crate::compiler::config_entries::resolve::resolve_mcp_collisions_for_target;

    let target_root = adapter.name();
    let mut entries = Vec::new();
    for parsed in resolve_mcp_collisions_for_target(mcp.iter().copied(), target_root, diag) {
        let entry = TargetMcpEntry::from_parsed(parsed);
        entries.push(ConfigEntry::McpServer(McpServerEntry {
            name: entry.name,
            command: entry.command,
            args: entry.args,
            env: entry.env.into_iter().collect(),
        }));
    }

    let Some(crate::target::HookFragmentMode::MergeJson {
        events: known_events,
    }) = adapter.hook_fragment_mode()
    else {
        return entries;
    };
    let mut contributions = Vec::new();
    for item in hooks.iter().copied() {
        match crate::compiler::hooks::load_merge_fragment(
            item,
            target_root,
            known_events,
            &hook_dir(item),
            diag,
            false,
        ) {
            Ok(fragment) => contributions.extend(
                fragment
                    .events
                    .into_iter()
                    .filter(|(_, entries)| !entries.is_empty())
                    .map(|(event, entries)| (item, event, entries)),
            ),
            Err(error) => diag.error("hook-fragment", error.to_string()),
        }
    }

    contributions.sort_by(|(left, left_event, _), (right, right_event, _)| {
        (
            left.package_depth,
            left.decl_order,
            left.def.order,
            left.def.name.as_str(),
            left_event.as_str(),
        )
            .cmp(&(
                right.package_depth,
                right.decl_order,
                right.def.order,
                right.def.name.as_str(),
                right_event.as_str(),
            ))
    });
    entries.extend(
        contributions
            .into_iter()
            .map(|(item, event, hook_entries)| {
                ConfigEntry::Hook(HookEntry {
                    name: item.def.name.clone(),
                    native_event: event,
                    entries: hook_entries,
                })
            }),
    );
    entries
}

fn validate_destination_path(
    root: &std::path::Path,
    destination: &std::path::Path,
    directory: bool,
) -> Result<(), String> {
    let relative = destination.strip_prefix(root).unwrap_or(destination);
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let is_destination = index + 1 == components.len();
        let Ok(metadata) = std::fs::metadata(&current) else {
            continue;
        };
        let expects_directory = !is_destination || directory;
        if metadata.is_dir() != expects_directory {
            let expected = if expects_directory {
                "directory"
            } else {
                "file"
            };
            return Err(format!(
                "destination `{}` must be a {expected}",
                current.display()
            ));
        }
    }
    Ok(())
}

/// Post-target-sync config-entry compilation: MCP servers and hooks.
///
/// MCP servers and hooks come from the target build's filter- and
/// collision-resolved selection.
///
/// Then:
/// 1. Run env-ref preflight (warn on missing vars)
/// 2. Detect per-target-root MCP name collisions
/// 3. Order hooks deterministically
/// 4. For each target root, lower items and write via adapter `write_config_entries()`
///
/// All errors are non-fatal — emitted as diagnostics and compilation continues.
pub(crate) fn compile_config_entries(
    ctx: &MarsContext,
    applied: &AppliedState,
    ownership_lock: &mut crate::lock::LockFile,
    dry_run: bool,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> ConfigEntryCompilation {
    use crate::compiler::mcp::check_env_refs;
    use crate::target::TargetRegistry;

    let effective = &applied.planned.targeted.resolved.loaded.effective;
    let target_roots: Vec<String> = effective.settings.managed_targets();
    let old_lock = &applied.planned.targeted.resolved.loaded.old_lock;

    // Env ref preflight (non-strict by default).
    let selected_mcp = applied
        .planned
        .targeted
        .target
        .selected_mcp()
        .cloned()
        .collect::<Vec<_>>();
    if let Err(e) = check_env_refs(&selected_mcp, false, diag) {
        diag.warn("mcp-env", format!("MCP env check failed: {e}"));
    }

    // Get the target registry.
    let registry = TargetRegistry::new();
    let mut desired_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> =
        BTreeMap::new();
    let mut pending_writes: BTreeMap<
        (String, crate::surface_ownership::retention::Surface),
        Vec<ConfigEntry>,
    > = BTreeMap::new();
    let mut pending_file_writes: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    let mut desired_file_outputs = std::collections::BTreeSet::new();
    let emitted_outputs = Vec::new();
    let mut removed_outputs = Vec::new();

    // First lower every target without mutating its config. Stale hook removal
    // is name-based across native events, so it must run before replacement
    // bindings with the same name are written.
    for target_root in &target_roots {
        let Some(adapter) = registry.get(target_root) else {
            continue;
        };
        let mode = adapter.hook_fragment_mode();
        let installed_hooks = applied
            .planned
            .targeted
            .target
            .selected_hooks_for_target(target_root)
            .filter(|(item, source_hash)| {
                let installed =
                    installed_hook_dir(&ctx.project_root, target_root, &item.def.name);
                if mode.is_some()
                    && !dry_run
                    && !hook_directory_is_installed(source_hash, &installed)
                {
                    diag.warn(
                        "hook-not-installed",
                        format!(
                            "not emitting hook `{}` for `{target_root}` because `{}` is not installed",
                            item.def.name,
                            installed.display()
                        ),
                    );
                    return false;
                }
                true
            })
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        let selected_mcp = applied
            .planned
            .targeted
            .target
            .selected_mcp_for_target(target_root)
            .collect::<Vec<_>>();
        let entries = lower_config_entries_for_target(
            &installed_hooks,
            &selected_mcp,
            adapter,
            |item| {
                portable_installed_hook_dir(&installed_hook_dir(
                    &ctx.project_root,
                    target_root,
                    &item.def.name,
                ))
            },
            diag,
        );

        // Resolve and load whole-file hooks independently of merge ordering.
        let mut file_writes = Vec::new();
        if mode == Some(crate::target::HookFragmentMode::File) {
            for item in &installed_hooks {
                let hook_dir = portable_installed_hook_dir(&installed_hook_dir(
                    &ctx.project_root,
                    target_root,
                    &item.def.name,
                ));
                let Some(relative_dest) = adapter.hook_file_dest_path(&item.def.name) else {
                    diag.error(
                        "hook-fragment",
                        format!(
                            "target `{target_root}` declares file-mode hooks without a placement path"
                        ),
                    );
                    continue;
                };
                match crate::compiler::hooks::load_file_fragment(item, target_root, &hook_dir) {
                    Ok(content) => {
                        let fragment = item.def.targets[target_root]
                            .fragment
                            .clone()
                            .unwrap_or_else(|| {
                                crate::compiler::hooks::default_fragment_name(target_root)
                            });
                        adapter.check_hook_fragment(
                            &item.def.name,
                            &item.source_name,
                            &fragment,
                            &content,
                            diag,
                        );
                        let relative_dest = relative_dest.to_string_lossy().into_owned();
                        desired_file_outputs.insert((target_root.clone(), relative_dest.clone()));
                        file_writes.push((
                            format!(
                                "hooks/{}/{}",
                                target_root.trim_start_matches('.'),
                                item.def.name
                            ),
                            relative_dest,
                            content,
                        ));
                    }
                    Err(error) => diag.error("hook-fragment", error.to_string()),
                }
            }
        }

        // Write via the target adapter (if one is registered).
        let mut target_records = BTreeMap::new();
        for entry in &entries {
            target_records.insert(entry.key(), adapter.config_entry_record(entry));
        }

        // Emit target-specific pre-write diagnostics (runs even on dry runs).
        adapter.emit_pre_write_diagnostics(&entries, diag);
        if !target_records.is_empty() {
            desired_records.insert(target_root.clone(), target_records);
        }
        for entry in entries {
            pending_writes
                .entry((target_root.clone(), entry.surface()))
                .or_default()
                .push(entry);
        }
        if !file_writes.is_empty() {
            pending_file_writes.insert(target_root.clone(), file_writes);
        }
    }

    let previous_records = &applied
        .planned
        .targeted
        .resolved
        .loaded
        .old_lock
        .config_entries;
    let removal_plan =
        crate::surface_ownership::retention::RemovalPlan::build(previous_records, &desired_records);

    if dry_run {
        for (target_root, keys) in removal_plan.stale_keys() {
            diag.warn(
                "stale-config-entry",
                format!(
                    "target `{target_root}` has stale config entries: {}",
                    keys.join(", ")
                ),
            );
        }
        return ConfigEntryCompilation {
            records: desired_records,
            emitted_outputs,
            removed_outputs,
        };
    }

    use crate::surface_ownership::retention::Surface;
    let retention = removal_plan.execute(
        |operation, diag| {
            let target_root = operation.target_root().to_owned();
            let surface = operation.surface();
            let Some(adapter) = registry.get(&target_root) else {
                let (_, removal) = operation.into_parts(&ctx.project_root);
                return crate::surface_ownership::retention::RemovalReport::failed(
                    format!("no adapter registered for `{target_root}`"),
                    removal.prior_records.clone(),
                );
            };
            match surface {
                Surface::Hook => adapter
                    .remove_owned_hook_entries(operation, &ctx.project_root, diag)
                    .context(format!(
                        "failed to remove prior hook entries from `{target_root}`"
                    )),
                Surface::Mcp => adapter
                    .remove_config_entries(operation, &ctx.project_root)
                    .context(format!(
                        "failed to remove stale config entries from `{target_root}`"
                    )),
            }
        },
        diag,
    );

    // File-mode fragments are ordinary target outputs. Remove only exact paths
    // owned by the old lock and no longer desired.
    for (target_root, paths) in file_hook_output_preserve_paths(old_lock) {
        for dest_path in paths {
            let pair = (target_root.clone(), dest_path.clone());
            if desired_file_outputs.contains(&pair) {
                continue;
            }
            if !crate::surface_ownership::may_delete(old_lock, &target_root, &dest_path) {
                continue;
            }
            let path = ctx.project_root.join(&target_root).join(&dest_path);
            match std::fs::remove_file(&path) {
                Ok(()) => removed_outputs.push(pair),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    removed_outputs.push(pair)
                }
                Err(error) => diag.warn(
                    "config-entry-remove",
                    format!("failed to remove `{}`: {error}", path.display()),
                ),
            }
        }
    }

    apply_replacement_writes(
        ctx,
        &registry,
        old_lock,
        ownership_lock,
        retention,
        pending_writes,
        pending_file_writes,
        &desired_records,
        force,
        diag,
        emitted_outputs,
        removed_outputs,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_replacement_writes(
    ctx: &MarsContext,
    registry: &crate::target::TargetRegistry,
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    retention: crate::surface_ownership::retention::RetentionPlan,
    pending_writes: BTreeMap<
        (String, crate::surface_ownership::retention::Surface),
        Vec<crate::target::ConfigEntry>,
    >,
    pending_file_writes: BTreeMap<String, Vec<(String, String, String)>>,
    desired_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    force: bool,
    diag: &mut DiagnosticCollector,
    mut emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    removed_outputs: Vec<(String, String)>,
) -> ConfigEntryCompilation {
    use crate::surface_ownership::retention::Surface;
    use crate::target::ConfigEntry;

    // Write desired entries only after every removal has completed.
    let mut written_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> =
        BTreeMap::new();
    for ((target_root, surface), entries) in pending_writes {
        let Some(permit) = retention.write_permit(&target_root, surface) else {
            diag.info(
                "config-entry-suppressed",
                format!(
                    "not writing {surface:?} entries to `{target_root}`: prior removal unconfirmed"
                ),
            );
            continue;
        };
        let write = match permit.bind_config_entries(entries) {
            Ok(write) => write,
            Err(error) => {
                diag.warn("config-entry-write", error.to_string());
                continue;
            }
        };
        let authorized_target = write.target_root().to_owned();
        let Some(adapter) = registry.get(write.target_root()) else {
            continue;
        };
        let written_keys: std::collections::BTreeSet<_> =
            write.entries().iter().map(ConfigEntry::key).collect();
        match adapter.write_config_entries(write, &ctx.project_root) {
            Ok(_) => {
                if let Some(records) = desired_records.get(&authorized_target) {
                    written_records
                        .entry(authorized_target.clone())
                        .or_default()
                        .extend(
                            records
                                .iter()
                                .filter(|(key, _)| written_keys.contains(*key))
                                .map(|(key, record)| (key.clone(), record.clone())),
                        );
                }
            }
            Err(error) => diag.warn(
                "config-entry-write",
                format!("failed to write config entries to `{authorized_target}`: {error}"),
            ),
        }
    }

    for (target_root, files) in pending_file_writes {
        let Some(permit) = retention.write_permit(&target_root, Surface::Hook) else {
            diag.info(
                "config-entry-suppressed",
                format!("not writing Hook entries to `{target_root}`: prior removal unconfirmed"),
            );
            continue;
        };
        let write = permit
            .bind_file_hooks(files)
            .expect("Hook permit accepts file-hook payloads");
        write_file_hook_outputs(
            write,
            ctx,
            old_lock,
            ownership_lock,
            force,
            diag,
            &mut emitted_outputs,
        );
    }

    let mut records = retention.into_retained_records();
    for (target_root, target_records) in written_records {
        records
            .entry(target_root)
            .or_default()
            .extend(target_records);
    }
    ConfigEntryCompilation {
        records,
        emitted_outputs,
        removed_outputs,
    }
}

#[allow(clippy::too_many_arguments)]
fn write_file_hook_outputs(
    write: crate::surface_ownership::retention::FileHookWrite<'_>,
    ctx: &MarsContext,
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    force: bool,
    diag: &mut DiagnosticCollector,
    emitted_outputs: &mut Vec<crate::lock::CompiledNativeOutput>,
) {
    let target_root = write.target_root().to_owned();
    let (target_dir, files) = write.into_parts(&ctx.project_root);
    for (owner_canonical_dest_path, relative, content) in files {
        let path = target_dir.join(&relative);
        let dest_exists = crate::surface_ownership::target_dest_exists(&path);
        match crate::surface_ownership::copy_decision(
            old_lock,
            &target_root,
            &relative,
            dest_exists,
            force,
        ) {
            crate::surface_ownership::SurfaceCopyDecision::SkipWithoutInstalledClaim => {
                crate::surface_ownership::warn_no_installed_claim_collision(
                    &target_root,
                    &relative,
                    crate::surface_ownership::CollisionAdoptHint::SyncForce,
                    diag,
                );
                continue;
            }
            crate::surface_ownership::SurfaceCopyDecision::Proceed => {
                if dest_exists
                    && force
                    && old_lock
                        .installed_checksum_for_output(&target_root, &relative)
                        .is_none()
                {
                    crate::surface_ownership::warn_no_installed_claim_adopted(
                        &target_root,
                        &relative,
                        crate::surface_ownership::CollisionAdoptHint::SyncForce,
                        diag,
                    );
                }
            }
        }
        if dest_exists && !force {
            let previous = old_lock.items.values().find_map(|item| {
                item.outputs.iter().find(|output| {
                    output.target_root == target_root
                        && crate::target::dest_paths_equivalent(
                            output.dest_path.as_str(),
                            &relative,
                        )
                })
            });
            let diverged = previous.is_some_and(|previous| {
                std::fs::read(&path)
                    .map(|bytes| crate::hash::hash_bytes(&bytes))
                    .is_ok_and(|actual| {
                        previous
                            .installed_checksum()
                            .is_none_or(|checksum| actual != checksum.as_ref())
                    })
            });
            if diverged {
                diag.warn(
                    "target-divergent",
                    format!(
                        "target `{target_root}` item `{relative}` was edited after Mars installed it \
                         (preserved local content; run `{}` to reset)",
                        crate::types::managed_cmd("mars sync --force")
                    ),
                );
                continue;
            }
        }
        let output = crate::lock::CompiledNativeOutput {
            owner_canonical_dest_path,
            target_root: target_root.clone(),
            dest_path: relative,
            installed_checksum: crate::types::ContentHash::from(crate::hash::hash_bytes(
                content.as_bytes(),
            )),
        };
        if let Err(error) = crate::lock::apply_compiled_native_outputs(
            ownership_lock,
            std::slice::from_ref(&output),
        ) {
            diag.warn(
                "config-entry-write",
                format!(
                    "not writing file hook `{}` because its ownership cannot be recorded: {error}",
                    path.display()
                ),
            );
            continue;
        }
        let result = (|| -> Result<(), crate::error::MarsError> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::fs::atomic_write(&path, content.as_bytes())
        })();
        if let Err(error) = result {
            diag.warn(
                "config-entry-write",
                format!("failed to write file hook `{}`: {error}", path.display()),
            );
            continue;
        }
        emitted_outputs.push(output);
    }
}

fn hook_directory_is_installed(
    source_hash: &crate::types::ContentHash,
    installed: &std::path::Path,
) -> bool {
    crate::hash::compute_hash(installed, crate::lock::ItemKind::Hook)
        .ok()
        .is_some_and(|installed| installed == source_hash.as_str())
}

fn installed_hook_dir(project_root: &Path, target_root: &str, hook_name: &str) -> PathBuf {
    project_root.join(target_root).join("hooks").join(hook_name)
}

fn portable_installed_hook_dir(installed: &std::path::Path) -> String {
    installed.to_string_lossy().replace('\\', "/")
}
