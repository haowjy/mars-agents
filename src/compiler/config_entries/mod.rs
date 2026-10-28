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
use crate::lock::ConfigEntryRecord;
use crate::surface_ownership::retention::{RemovalOperation, RemovalReport};
use crate::sync::AppliedState;
use crate::sync::package_dir::PackageDir;
use crate::target::{
    HOOK_KEY_PREFIX, HookEntry, MCP_KEY_PREFIX, McpServerEntry, TargetAdapter, TargetRegistry,
    mcp_file,
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
    // Targets whose MCP file / hook files this sync reads: those the old lock
    // has records on, and those with something selected.
    let mut mcp_targets: BTreeSet<String> = BTreeSet::new();
    let mut hook_targets: BTreeSet<String> = BTreeSet::new();
    for (target, records) in &resolved.loaded.old_lock.config_entries {
        if records.keys().any(|key| key.starts_with(MCP_KEY_PREFIX)) {
            mcp_targets.insert(target.clone());
        }
        if records.keys().any(|key| key.starts_with(HOOK_KEY_PREFIX)) {
            hook_targets.insert(target.clone());
        }
    }
    hook_targets.extend(
        targeted
            .target
            .selected_hooks()
            .map(|(dir, _)| dir.target().to_string()),
    );
    mcp_targets.extend(targeted.target.selected_mcp_targets().map(str::to_string));
    for target_name in mcp_targets.union(&hook_targets) {
        let touches_mcp = mcp_targets.contains(target_name);
        let touches_hooks = hook_targets.contains(target_name);
        let target_dir = ctx.project_root.join(target_name);
        if let Err(error) = validate_destination_path(&ctx.project_root, &target_dir, true) {
            errors.push(error);
            continue;
        }
        if let Some(adapter) = registry.get(target_name) {
            if touches_mcp && let Some(file) = adapter.mcp_file() {
                let location = &file.at;
                let path = ctx.project_root.join(location.path);
                if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                    errors.push(error);
                } else if let Err(error) = crate::target::mcp_file::validate(location, &path) {
                    errors.push(format!(
                        "target `{target_name}` reads MCP servers from `{}`: {}",
                        location.path,
                        error_message(error)
                    ));
                }
            }
            let names = if touches_hooks {
                adapter.hook_config_file_names()
            } else {
                &[]
            };
            for name in names {
                let path = target_dir.join(name);
                if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                    errors.push(error);
                }
                if let Err(error) = crate::target::hook_file::validate(&path) {
                    errors.push(error_message(error));
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
            errors.push(error_message(error));
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

/// An error's message without its `config error:` prefix: preflight wraps the
/// joined messages in one `ConfigError::Invalid`, which supplies it.
fn error_message(error: crate::error::MarsError) -> String {
    match error {
        crate::error::MarsError::Config(crate::error::ConfigError::Invalid { message }) => message,
        other => other.to_string(),
    }
}

/// Lower the selected MCP servers for one target.
///
/// The caller owns eligibility checks, including whether an installed MCP
/// directory is current, and supplies each item's directory token with it: the
/// replacement for `${MARS_MCP_DIR}` in a server's command and arguments.
pub(crate) fn lower_mcp_servers(mcp: &[(String, &ParsedMcpItem)]) -> Vec<McpServerEntry> {
    mcp.iter()
        .map(|(dir, parsed)| McpServerEntry {
            name: parsed.name.clone(),
            transport: lower_transport(parsed, dir),
        })
        .collect()
}

/// Lower the mergeable hook entries for one target.
///
/// The caller supplies each hook's directory token: the replacement for
/// `${MARS_HOOK_DIR}` in its native fragment.
pub(crate) fn lower_hook_entries(
    hooks: &[(String, &ParsedHookItem)],
    adapter: &dyn TargetAdapter,
    diag: &mut DiagnosticCollector,
) -> Vec<HookEntry> {
    let target_root = adapter.name();
    let Some(crate::target::HookFragmentMode::MergeJson {
        events: known_events,
    }) = adapter.hook_fragment_mode()
    else {
        return Vec::new();
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
    contributions
        .into_iter()
        .map(|(item, event, entries)| HookEntry {
            name: item.def.name.clone(),
            native_event: event,
            entries,
        })
        .collect()
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
/// 4. For each target root, lower items; hooks are removed then appended via the
///    adapter, MCP servers are reconciled in one pass per target file
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
    let mut desired_mcp: BTreeMap<String, Vec<mcp_file::Rendered>> = BTreeMap::new();
    let mut pending_hooks: BTreeMap<String, Vec<HookEntry>> = BTreeMap::new();
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
        let hooks = lower_hook_entries(&hook_entries, adapter, diag);
        let mcp_servers = lower_mcp_servers(&mcp_entries);

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

        // Render each server once. Lossiness warnings come with the render, so
        // they run even on dry runs.
        let rendered = adapter
            .mcp_file()
            .map(|file| mcp_file::render_all(&file, &mcp_servers, diag))
            .unwrap_or_default();
        let target_records: BTreeMap<_, _> = rendered
            .iter()
            .map(|server| (server.key(), server.record()))
            .chain(hooks.iter().map(|hook| {
                (
                    hook.key(),
                    ConfigEntryRecord {
                        emitted_json: serde_json::to_string(&hook.entries)
                            .expect("JSON values serialize"),
                    },
                )
            }))
            .collect();
        if !target_records.is_empty() {
            desired_records.insert(target_root.clone(), target_records);
        }
        if !rendered.is_empty() {
            desired_mcp.insert(target_root.clone(), rendered);
        }
        if !hooks.is_empty() {
            pending_hooks.insert(target_root.clone(), hooks);
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

    if dry_run {
        for (target_root, records) in previous_records {
            let stale: Vec<&str> = records
                .keys()
                .filter(|key| {
                    !desired_records
                        .get(target_root)
                        .is_some_and(|desired| desired.contains_key(*key))
                })
                .map(String::as_str)
                .collect();
            if !stale.is_empty() {
                diag.warn(
                    "stale-config-entry",
                    format!(
                        "target `{target_root}` has stale config entries: {}",
                        stale.join(", ")
                    ),
                );
            }
        }
        return ConfigEntryCompilation {
            records: desired_records,
            emitted_outputs,
            removed_outputs,
        };
    }

    let retention =
        crate::surface_ownership::retention::RemovalPlan::build(previous_records, &desired_records)
            .execute(
                |operation, diag| remove_hooks(&registry, operation, &ctx.project_root, diag),
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

    // MCP servers are reconciled per target file: a target no longer managed
    // still gets its stale servers removed.
    let mut mcp_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> = BTreeMap::new();
    let mut mcp_removed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for target_root in previous_records
        .keys()
        .chain(desired_mcp.keys())
        .collect::<BTreeSet<_>>()
    {
        let reconciled = reconcile_mcp(
            &registry,
            &ctx.project_root,
            target_root,
            previous_records.get(target_root),
            desired_mcp.get(target_root).map_or(&[], Vec::as_slice),
            force,
            diag,
        );
        if !reconciled.records.is_empty() {
            mcp_records.insert(target_root.clone(), reconciled.records);
        }
        mcp_removed.insert(target_root.clone(), reconciled.removed);
    }
    report_removed(retention.removed_keys(), &mcp_removed, diag);

    let mut compiled = apply_replacement_writes(
        ctx,
        &registry,
        old_lock,
        ownership_lock,
        retention,
        pending_hooks,
        pending_file_writes,
        &desired_records,
        force,
        diag,
        emitted_outputs,
        removed_outputs,
    );
    for (target_root, records) in mcp_records {
        compiled
            .records
            .entry(target_root)
            .or_default()
            .extend(records);
    }
    compiled
}

/// Remove the hook entries `operation` names. Shared by sync and `mars unlink`.
pub(crate) fn remove_hooks(
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
    adapter
        .remove_owned_hook_entries(operation, project_root, diag)
        .context(format!(
            "failed to remove prior hook entries from `{target_root}`"
        ))
}

/// Report the stale entries a sync or `mars unlink` took off the books: the
/// hooks the retention plan removed and the MCP servers `reconcile_mcp` did,
/// as one line per target.
pub(crate) fn report_removed(
    hooks: &BTreeMap<String, Vec<String>>,
    mcp: &BTreeMap<String, Vec<String>>,
    diag: &mut DiagnosticCollector,
) {
    for target_root in hooks.keys().chain(mcp.keys()).collect::<BTreeSet<_>>() {
        let keys: Vec<&str> = [hooks.get(target_root), mcp.get(target_root)]
            .into_iter()
            .flatten()
            .flatten()
            .map(String::as_str)
            .collect();
        if keys.is_empty() {
            continue;
        }
        diag.info(
            "stale-config-entry",
            format!(
                "removed stale config entries from `{target_root}`: {}",
                keys.join(", ")
            ),
        );
    }
}

/// Reconcile `target_root`'s MCP file with `desired`. Shared by sync and
/// `mars unlink` (`desired` empty).
pub(crate) fn reconcile_mcp(
    registry: &TargetRegistry,
    project_root: &Path,
    target_root: &str,
    previous: Option<&BTreeMap<String, ConfigEntryRecord>>,
    desired: &[mcp_file::Rendered],
    force: bool,
    diag: &mut DiagnosticCollector,
) -> mcp_file::Reconciled {
    let no_records = BTreeMap::new();
    let previous = previous.unwrap_or(&no_records);
    let Some(adapter) = registry.get(target_root) else {
        let retained: BTreeMap<_, _> = previous
            .iter()
            .filter(|(key, _)| key.starts_with(MCP_KEY_PREFIX))
            .map(|(key, record)| (key.clone(), record.clone()))
            .collect();
        if !retained.is_empty() {
            diag.warn(
                "config-entry-remove",
                format!(
                    "failed to remove stale config entries from `{target_root}`: no adapter registered for `{target_root}`"
                ),
            );
        }
        return mcp_file::Reconciled {
            records: retained,
            removed: Vec::new(),
        };
    };
    // A target that takes no MCP servers has no file to hold a record.
    let Some(file) = adapter.mcp_file() else {
        return mcp_file::Reconciled {
            records: BTreeMap::new(),
            removed: Vec::new(),
        };
    };
    mcp_file::reconcile(
        &file,
        project_root,
        target_root,
        previous,
        desired,
        force,
        diag,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_replacement_writes(
    ctx: &MarsContext,
    registry: &crate::target::TargetRegistry,
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    retention: crate::surface_ownership::retention::RetentionPlan,
    pending_hooks: BTreeMap<String, Vec<HookEntry>>,
    pending_file_writes: BTreeMap<String, Vec<(String, String, String)>>,
    desired_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    force: bool,
    diag: &mut DiagnosticCollector,
    mut emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    removed_outputs: Vec<(String, String)>,
) -> ConfigEntryCompilation {
    // Write desired hooks only after every removal has completed. Removal
    // cleared the target's recorded hooks, so a failed write leaves no record.
    let mut written_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> =
        BTreeMap::new();
    for (target_root, hooks) in pending_hooks {
        let Some(permit) = retention.write_permit(&target_root) else {
            diag.info(
                "config-entry-suppressed",
                format!("not writing Hook entries to `{target_root}`: prior removal unconfirmed"),
            );
            continue;
        };
        let write = permit.bind_hooks(hooks);
        let Some(adapter) = registry.get(write.target_root()) else {
            continue;
        };
        let keys: Vec<String> = write.hooks().iter().map(HookEntry::key).collect();
        if let Err(error) = adapter.write_hook_entries(write, &ctx.project_root) {
            diag.warn(
                "config-entry-write",
                format!("failed to write config entries to `{target_root}`: {error}"),
            );
            continue;
        }
        written_records
            .entry(target_root.clone())
            .or_default()
            .extend(
                desired_records
                    .get(&target_root)
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| keys.contains(key))
                    .map(|(key, record)| (key.clone(), record.clone())),
            );
    }

    for (target_root, files) in pending_file_writes {
        let Some(permit) = retention.write_permit(&target_root) else {
            diag.info(
                "config-entry-suppressed",
                format!("not writing Hook entries to `{target_root}`: prior removal unconfirmed"),
            );
            continue;
        };
        let write = permit.bind_file_hooks(files);
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
