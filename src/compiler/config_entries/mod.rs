//! Config-entry compiler lane for MCP servers and hooks.
//!
//! This module owns config-entry lowering and writes. MCP and hook selection
//! come from the target build that also selects installed content.

pub mod resolve;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::compiler::hooks::ParsedHookItem;
use crate::compiler::mcp::{ParsedMcpItem, lower_transport};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::ConfigEntryRecord;
use crate::surface_ownership::retention::{ConfigWrite, RemovalOperation, RemovalReport, Surface};
use crate::sync::AppliedState;
use crate::sync::package_dir::PackageDir;
use crate::target::{
    ConfigEntry, HookEntry, McpServerEntry, TargetAdapter, TargetRegistry, mcp_file,
};
use crate::types::MarsContext;
use mars_schema::item::ItemKind;

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
        .filter(|item| item.kind == ItemKind::Hook)
    {
        for output in item
            .outputs
            .iter()
            .filter(|output| output.target_root != crate::lock::CANONICAL_TARGET_ROOT)
        {
            let Some(adapter) = registry.get(&output.target_root) else {
                continue;
            };
            // The canonical record of a hook installed as one file names its
            // package directory; the file that directory projects must stay.
            let is_file_fragment = item.outputs.iter().any(|canonical| {
                canonical.target_root == crate::lock::CANONICAL_TARGET_ROOT
                    && PackageDir::parse_canonical(ItemKind::Hook, &canonical.dest_path)
                        .filter(|dir| dir.belongs_to(&output.target_root))
                        .and_then(|dir| adapter.hook_file_dest_path(dir.name()))
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
    for (dir, _) in targeted.target.selected_hooks() {
        touched_targets
            .entry(dir.target().to_string())
            .or_default()
            .1 = true;
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
            let records = resolved.loaded.old_lock.config_entries.get(target_name);
            let has_legacy_hooks = has_unrecorded(records, Surface::Hook);
            if *touches_mcp && let Some(file) = adapter.mcp_file() {
                let has_legacy = has_unrecorded(records, Surface::Mcp);
                for location in
                    std::iter::once(&file.at).chain(file.legacy.iter().filter(|_| has_legacy))
                {
                    let path = ctx.project_root.join(location.path);
                    if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                        errors.push(error);
                    } else if let Err(error) = crate::target::mcp_file::validate(location, &path) {
                        errors.push(error.to_string());
                    }
                }
            }
            let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
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
    for (dir, item) in targeted.target.selected_hooks() {
        let target_name = dir.target();
        let adapter = registry.get(target_name);
        let mode = adapter.and_then(|adapter| adapter.hook_fragment_mode());
        let hook_dir = installed_token(ctx, dir);
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
/// The caller owns target-specific eligibility checks, including whether an
/// installed hook or MCP directory is current, and supplies each item's
/// directory token with it: the replacement for `${MARS_HOOK_DIR}` in a hook's
/// native fragment and for `${MARS_MCP_DIR}` in a server's command and
/// arguments.
pub(crate) fn lower_config_entries_for_target(
    hooks: &[(String, &ParsedHookItem)],
    mcp: &[(String, &ParsedMcpItem)],
    adapter: &dyn TargetAdapter,
    diag: &mut DiagnosticCollector,
) -> Vec<ConfigEntry> {
    let target_root = adapter.name();
    let mut entries = Vec::new();
    for (dir, parsed) in mcp {
        entries.push(ConfigEntry::McpServer(McpServerEntry {
            name: parsed.name.clone(),
            transport: lower_transport(parsed, dir),
        }));
    }

    let Some(crate::target::HookFragmentMode::MergeJson {
        events: known_events,
    }) = adapter.hook_fragment_mode()
    else {
        return entries;
    };
    let mut contributions = Vec::new();
    for (dir, item) in hooks {
        match crate::compiler::hooks::load_merge_fragment(
            item,
            target_root,
            known_events,
            dir,
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
/// 4. For each target root, lower items and write via adapter `write_hook_entries()`
///
/// All errors are non-fatal — emitted as diagnostics and compilation continues.
pub(crate) fn compile_config_entries(
    ctx: &MarsContext,
    applied: &AppliedState,
    ownership_lock: &mut crate::lock::LockFile,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
    dry_run: bool,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> ConfigEntryCompilation {
    use crate::compiler::mcp::check_env_refs;
    use crate::target::TargetRegistry;

    let effective = &applied.planned.targeted.resolved.loaded.effective;
    let target_roots: Vec<String> = effective.settings.managed_targets();
    let old_lock = &applied.planned.targeted.resolved.loaded.old_lock;

    // Env ref preflight (non-strict by default). A server selected for several
    // targets is one server to check.
    let mut checked_mcp = std::collections::HashSet::new();
    let selected_mcp = applied
        .planned
        .targeted
        .target
        .selected_mcp()
        .map(|(_, item)| item)
        .filter(|item| checked_mcp.insert((&item.source_name, &item.source_path, &item.name)))
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
            .filter(|(dir, _)| emits_installed_copy(ctx, dir, target_outcomes, dry_run, diag))
            .collect::<Vec<_>>();
        let selected_mcp = applied
            .planned
            .targeted
            .target
            .selected_mcp_for_target(target_root)
            .filter(|(dir, item)| {
                // A server whose `command`/`args` name no directory has nothing
                // to wait for: its entry is all it installs.
                !item.uses_dir_token()
                    || emits_installed_copy(ctx, dir, target_outcomes, dry_run, diag)
            })
            .collect::<Vec<_>>();
        let hook_entries = installed_hooks
            .iter()
            .map(|(dir, item)| (installed_token(ctx, dir), *item))
            .collect::<Vec<_>>();
        let mcp_entries = selected_mcp
            .iter()
            .map(|(dir, item)| (installed_token(ctx, dir), *item))
            .collect::<Vec<_>>();
        let entries = lower_config_entries_for_target(&hook_entries, &mcp_entries, adapter, diag);

        // Resolve and load whole-file hooks independently of merge ordering.
        let mut file_writes = Vec::new();
        if mode == Some(crate::target::HookFragmentMode::File) {
            for (dir, item) in &installed_hooks {
                let hook_dir = installed_token(ctx, dir);
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
                        if let Some(problem) = adapter.hook_fragment_problem(&content) {
                            diag.warn(
                                "hook-fragment-contract",
                                format!(
                                    "hook `{}` from `{}`: `{target_root}` fragment `{}` {problem}; the fragment will still be installed",
                                    item.def.name,
                                    item.source_name,
                                    item.fragment_name(target_root),
                                ),
                            );
                        }
                        let relative_dest = relative_dest.to_string_lossy().into_owned();
                        desired_file_outputs.insert((target_root.clone(), relative_dest.clone()));
                        file_writes.push((
                            dir.canonical().as_str().to_string(),
                            relative_dest,
                            content,
                        ));
                    }
                    Err(error) => diag.error("hook-fragment", error.to_string()),
                }
            }
        }

        // Write via the target adapter (if one is registered).
        // Lossiness warnings come with the records, so they run even on dry runs.
        let mut target_records = BTreeMap::new();
        for entry in &entries {
            target_records.insert(entry.key(), record_entry(adapter, entry, diag));
        }
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

    let retention = removal_plan.execute(
        |operation, diag| remove(&registry, operation, &ctx.project_root, diag),
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

/// Whether `records` hold an entry of `surface` from before content ownership
/// (no `emitted_json`), which preflight must be able to clean up.
fn has_unrecorded(records: Option<&BTreeMap<String, ConfigEntryRecord>>, surface: Surface) -> bool {
    records.is_some_and(|records| {
        records
            .iter()
            .any(|(key, record)| Surface::of_key(key) == surface && record.emitted_json.is_none())
    })
}

/// The lock record for one emitted entry: an MCP server records its native
/// rendering, a hook its structural entries. Warns about what an MCP server's
/// native shape cannot carry.
fn record_entry(
    adapter: &dyn TargetAdapter,
    entry: &ConfigEntry,
    diag: &mut DiagnosticCollector,
) -> ConfigEntryRecord {
    match entry {
        ConfigEntry::Hook(hook) => ConfigEntryRecord {
            emitted_json: serde_json::to_string(&hook.entries).ok(),
        },
        ConfigEntry::McpServer(server) => {
            let file = adapter
                .mcp_file()
                .expect("MCP servers are lowered only for targets that declare an MCP file");
            mcp_file::record_and_warn(&file, server, diag)
        }
    }
}

/// Remove the entries `operation` names, on whichever surface it is for. Shared by
/// sync and `mars unlink`.
pub(crate) fn remove(
    registry: &TargetRegistry,
    operation: RemovalOperation<'_>,
    project_root: &Path,
    diag: &mut DiagnosticCollector,
) -> RemovalReport {
    let target_root = operation.target_root().to_owned();
    let Some(adapter) = registry.get(&target_root) else {
        let (_, removal) = operation.into_parts(project_root);
        return RemovalReport::failed(
            format!("no adapter registered for `{target_root}`"),
            removal.prior_records.clone(),
        );
    };
    match operation.surface() {
        Surface::Hook => adapter
            .remove_owned_hook_entries(operation, project_root, diag)
            .context(format!(
                "failed to remove prior hook entries from `{target_root}`"
            )),
        Surface::Mcp => mcp_file::remove_entries(adapter, operation, project_root, diag).context(
            format!("failed to remove stale config entries from `{target_root}`"),
        ),
    }
}

/// Write `write`'s entries on `surface`; returns the keys now on disk as rendered.
fn write_entries(
    adapter: &dyn TargetAdapter,
    surface: Surface,
    write: ConfigWrite<'_>,
    project_root: &Path,
    previous: Option<&BTreeMap<String, ConfigEntryRecord>>,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> Result<BTreeSet<String>, MarsError> {
    match surface {
        Surface::Hook => {
            let keys = write.entries().iter().map(ConfigEntry::key).collect();
            adapter.write_hook_entries(write, project_root)?;
            Ok(keys)
        }
        Surface::Mcp => {
            let file = adapter
                .mcp_file()
                .expect("MCP servers are lowered only for targets that declare an MCP file");
            let target_root = write.target_root().to_owned();
            mcp_file::write_entries(
                &file,
                write,
                project_root,
                &mcp_file::WriteOwnership {
                    target_root: &target_root,
                    previous,
                    force,
                },
                diag,
            )
        }
    }
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
        let attempted: std::collections::BTreeSet<_> =
            write.entries().iter().map(ConfigEntry::key).collect();
        let result = write_entries(
            adapter,
            surface,
            write,
            &ctx.project_root,
            old_lock.config_entries.get(&authorized_target),
            force,
            diag,
        );
        let written_keys = result.unwrap_or_else(|error| {
            diag.warn(
                "config-entry-write",
                format!("failed to write config entries to `{authorized_target}`: {error}"),
            );
            BTreeSet::new()
        });
        let records = written_records
            .entry(authorized_target.clone())
            .or_default();
        records.extend(
            desired_records
                .get(&authorized_target)
                .into_iter()
                .flatten()
                .filter(|(key, _)| written_keys.contains(*key))
                .map(|(key, record)| (key.clone(), record.clone())),
        );
        // An MCP entry attempted but not written (preserved as the user's, or
        // the write failed) is still on disk as the old lock recorded it, so
        // keep that record. Hook removal clears the whole pair before the
        // write, so no hook record applies.
        if surface == Surface::Mcp {
            records.extend(
                old_lock
                    .config_entries
                    .get(&authorized_target)
                    .into_iter()
                    .flatten()
                    .filter(|(key, record)| {
                        attempted.contains(*key)
                            && !written_keys.contains(*key)
                            && record.emitted_json.is_some()
                    })
                    .map(|(key, record)| (key.clone(), record.clone())),
            );
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

/// Whether target sync installed `dir`'s copy, warning when it did not so the
/// caller withholds the config entry that would name a missing path. An
/// unmanaged path (preserved without an installed claim) and a user-edited copy
/// (preserved as `target-divergent`) both leave no install record.
fn emits_installed_copy(
    ctx: &MarsContext,
    dir: &PackageDir,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> bool {
    // A dry run performs no target sync, so there is no install record to read.
    if dry_run {
        return true;
    }
    let installed = target_outcomes.iter().any(|outcome| {
        outcome.target == dir.target()
            && outcome.synced_outputs.iter().any(|output| {
                crate::target::dest_paths_equivalent(&output.dest_path, &dir.native())
            })
    });
    if !installed {
        let (code, noun) = dir.not_installed_label();
        diag.warn(
            code,
            format!(
                "not emitting {noun} `{}` for `{}`: `{}` did not sync from `.mars`",
                dir.name(),
                dir.target(),
                installed_path(ctx, dir).display()
            ),
        );
    }
    installed
}

/// The path target sync installs `dir`'s copy at.
fn installed_path(ctx: &MarsContext, dir: &PackageDir) -> PathBuf {
    ctx.project_root.join(dir.target()).join(dir.native())
}

/// The `${MARS_HOOK_DIR}` or `${MARS_MCP_DIR}` replacement for a native entry.
fn installed_token(ctx: &MarsContext, dir: &PackageDir) -> String {
    installed_path(ctx, dir)
        .to_string_lossy()
        .replace('\\', "/")
}
