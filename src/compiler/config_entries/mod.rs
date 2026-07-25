//! Config-entry compiler lane for MCP servers and hooks.
//!
//! This module owns discovery, filtering, lowering, and target-adapter writes
//! for package-defined MCP servers and hooks.

pub mod resolve;
use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::sync::AppliedState;
use crate::types::{MarsContext, SourceName};

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
    resolved: &crate::sync::ResolvedState,
    force: bool,
    diag: &mut DiagnosticCollector,
) -> Result<(), crate::error::MarsError> {
    use crate::compiler::hooks::{discover_hook_items, load_file_fragment, load_merge_fragment};
    use crate::error::{ConfigError, MarsError};
    use crate::target::{HookFragmentMode, TargetRegistry};

    let mut hooks = discover_hook_items(&ctx.project_root, "_self", 0, 0)?;
    for (decl_order, source_name) in resolved.graph.order.iter().enumerate() {
        if let Some(node) = resolved.graph.nodes.get(source_name) {
            hooks.extend(crate::compiler::hooks::discover_resolved_hook_items(
                node,
                source_name,
                1,
                decl_order,
            )?);
        }
    }
    let registry = TargetRegistry::new();
    let mut errors = Vec::new();
    let managed_targets = resolved.loaded.effective.settings.managed_targets();
    let mut touched_targets: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    for (target, records) in &resolved.loaded.old_lock.config_entries {
        let touch = touched_targets.entry(target.clone()).or_default();
        touch.0 |= records.keys().any(|key| key.starts_with("mcp:"));
        touch.1 |= records.keys().any(|key| key.starts_with("hook"));
    }
    for hook in &hooks {
        if hook.source_name == "_self" || hook.def.visibility == "exported" {
            for target in hook
                .def
                .targets
                .keys()
                .filter(|target| managed_targets.contains(target))
            {
                touched_targets.entry(target.clone()).or_default().1 = true;
            }
        }
    }
    let mut mcp_items =
        match crate::compiler::mcp::discover_mcp_items(&ctx.project_root, "_self", 0) {
            Ok(items) => items,
            Err(error) => {
                diag.warn(
                    "mcp-discover",
                    format!("failed to scan local MCP items: {error}"),
                );
                Vec::new()
            }
        };
    for (decl_order, source_name) in resolved.graph.order.iter().enumerate() {
        let Some(node) = resolved.graph.nodes.get(source_name) else {
            continue;
        };
        if !source_may_emit_mcp(&resolved.graph, source_name) {
            continue;
        }
        match crate::compiler::mcp::discover_mcp_items(
            &node.rooted_ref.package_root,
            source_name.as_str(),
            decl_order,
        ) {
            Ok(items) => mcp_items.extend(items),
            Err(error) => diag.warn(
                "mcp-discover",
                format!("failed to scan MCP items in `{source_name}`: {error}"),
            ),
        }
    }
    for item in mcp_items {
        if item.source_name != "_self" && item.def.visibility != "exported" {
            continue;
        }
        if item.def.targets.is_empty() {
            for target in &managed_targets {
                touched_targets.entry(target.clone()).or_default().0 = true;
            }
        } else {
            for target in item
                .def
                .targets
                .iter()
                .filter(|target| managed_targets.contains(target))
            {
                touched_targets.entry(target.clone()).or_default().0 = true;
            }
        }
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
    for item in hooks {
        if item.source_name != "_self" && item.def.visibility != "exported" {
            continue;
        }
        for target_name in item.def.targets.keys() {
            if !resolved
                .loaded
                .effective
                .settings
                .managed_targets()
                .contains(target_name)
            {
                continue;
            }
            let adapter = registry.get(target_name);
            let mode = adapter.and_then(|adapter| adapter.hook_fragment_mode());
            let installed = ctx
                .project_root
                .join(target_name)
                .join("hooks")
                .join(&item.def.name);
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
                Some(HookFragmentMode::MergeJson) => {
                    match adapter.and_then(|adapter| adapter.known_hook_events()) {
                        Some(known) => {
                            load_merge_fragment(&item, target_name, known, &installed, diag, true)
                                .map(|_| ())
                        }
                        None => Err(MarsError::Config(ConfigError::Invalid {
                            message: format!(
                                "hook `{}`: target `{target_name}` has no native event allowlist",
                                item.def.name
                            ),
                        })),
                    }
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
                    load_file_fragment(&item, target_name, &installed).map(|_| ())
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
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(MarsError::Config(ConfigError::Invalid {
            message: errors.join("\n"),
        }))
    }
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
/// For each package in the resolved graph:
/// 1. Discover MCP items from `mcp/<name>/mcp.toml`
/// 2. Discover hook items from `hooks/<name>/hook.toml`
///
/// Then:
/// 3. Run env-ref preflight (warn on missing vars)
/// 4. Detect per-target-root MCP name collisions
/// 5. Order hooks deterministically
/// 6. For each target root, lower items and write via adapter `write_config_entries()`
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
    use crate::compiler::config_entries::resolve::{
        LoadedHookContribution, resolve_file_hook_collisions_for_target,
        resolve_hook_collisions_for_target, resolve_mcp_collisions_for_target,
    };
    use crate::compiler::hooks::discover_hook_items;
    use crate::compiler::mcp::{TargetMcpEntry, check_env_refs, discover_mcp_items};
    use crate::target::{ConfigEntry, HookEntry, McpServerEntry, TargetRegistry};

    let graph = &applied.planned.targeted.resolved.graph;
    let effective = &applied.planned.targeted.resolved.loaded.effective;
    let target_roots: Vec<String> = effective.settings.managed_targets();
    let old_lock = &applied.planned.targeted.resolved.loaded.old_lock;

    // Compute package depths from direct deps (depth 1; local = 0).
    let depths = compute_depths(graph);
    // Compute declaration-order precedence from mars.toml insertion order.
    let decl_orders = compute_decl_orders(graph, &effective.dependencies);

    // Collect all MCP and hook items across all packages.
    let mut all_mcp: Vec<crate::compiler::mcp::ParsedMcpItem> = Vec::new();
    let mut all_hooks: Vec<crate::compiler::hooks::ParsedHookItem> = Vec::new();

    // Local package (depth 0, decl_order 0).
    let local_mcp = match discover_mcp_items(&ctx.project_root, "_self", 0) {
        Ok(items) => items,
        Err(e) => {
            diag.warn(
                "mcp-discover",
                format!("failed to scan local MCP items: {e}"),
            );
            Vec::new()
        }
    };
    all_mcp.extend(local_mcp);

    let local_hooks = match discover_hook_items(&ctx.project_root, "_self", 0, 0) {
        Ok(items) => items,
        Err(e) => {
            diag.error(
                "hook-discover",
                format!("failed to scan local hook items: {e}"),
            );
            Vec::new()
        }
    };
    all_hooks.extend(local_hooks);

    // Dependency packages.
    for source_name in &graph.order {
        let Some(node) = graph.nodes.get(source_name) else {
            continue;
        };
        let package_root = &node.rooted_ref.package_root;
        let decl_order = decl_orders
            .get(source_name)
            .copied()
            .unwrap_or(effective.dependencies.len() + graph.order.len() + 1);

        if source_may_emit_mcp(graph, source_name) {
            match discover_mcp_items(package_root, source_name.as_str(), decl_order) {
                Ok(items) => all_mcp.extend(items),
                Err(e) => {
                    diag.warn(
                        "mcp-discover",
                        format!("failed to scan MCP items in `{source_name}`: {e}"),
                    );
                }
            }
        }

        let depth = depths.get(source_name).copied().unwrap_or(1);
        match crate::compiler::hooks::discover_resolved_hook_items(
            node,
            source_name,
            depth,
            decl_order,
        ) {
            Ok(items) => all_hooks.extend(items),
            Err(e) => {
                diag.error(
                    "hook-discover",
                    format!("failed to scan hook items in `{source_name}`: {e}"),
                );
            }
        }
    }

    // HIGH-3: Filter out hooks and MCP items from dependency packages where visibility is Local.
    {
        use crate::compiler::visibility::{can_cross_package_boundary, resolve_visibility};
        use crate::lock::ItemKind;

        all_mcp.retain(|item| {
            // Local package items always pass.
            if item.source_name == "_self" {
                return true;
            }
            // Dependency item — check visibility.
            let explicit = match item.def.visibility.as_str() {
                "exported" => Some(true),
                "local" => Some(false),
                _ => None, // treat unknown as default (local)
            };
            let vis = resolve_visibility(ItemKind::McpServer, &item.name, explicit);
            if !can_cross_package_boundary(&vis) {
                return false;
            }
            // Emit warning for explicitly exported effectful items.
            true
        });

        all_hooks.retain(|item| {
            // Local package items always pass.
            if item.source_name == "_self" {
                return true;
            }
            // Dependency item — check visibility.
            let explicit = match item.def.visibility.as_str() {
                "exported" => Some(true),
                "local" => Some(false),
                _ => None,
            };
            let vis = resolve_visibility(ItemKind::Hook, &item.def.name, explicit);
            can_cross_package_boundary(&vis)
        });
    }

    // Env ref preflight (non-strict by default).
    if let Err(e) = check_env_refs(&all_mcp, false, diag) {
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
        // Lower MCP items for this target.
        let mut entries = Vec::new();

        for parsed in resolve_mcp_collisions_for_target(&all_mcp, target_root, diag) {
            let e = TargetMcpEntry::from_parsed(parsed);
            entries.push(ConfigEntry::McpServer(McpServerEntry {
                name: e.name,
                command: e.command,
                args: e.args,
                env: e.env.into_iter().collect(),
            }));
        }

        // Load opaque fragment arrays, then collision-resolve by (event, name).
        let Some(adapter) = registry.get(target_root) else {
            continue;
        };
        let mode = adapter.hook_fragment_mode();
        let mut contributions = Vec::new();
        if mode == Some(crate::target::HookFragmentMode::MergeJson) {
            let Some(known_events) = adapter.known_hook_events() else {
                continue;
            };
            for item in all_hooks
                .iter()
                .filter(|item| item.def.targets.contains_key(target_root))
            {
                let installed = ctx
                    .project_root
                    .join(target_root)
                    .join("hooks")
                    .join(&item.def.name);
                if !dry_run && !hook_directory_is_installed(item, &installed) {
                    diag.warn(
                        "hook-not-installed",
                        format!(
                            "not emitting hook `{}` for `{target_root}` because `{}` is not installed",
                            item.def.name,
                            installed.display()
                        ),
                    );
                    continue;
                }
                match crate::compiler::hooks::load_merge_fragment(
                    item,
                    target_root,
                    known_events,
                    &installed,
                    diag,
                    false,
                ) {
                    Ok(fragment) => contributions.extend(
                        fragment
                            .events
                            .into_iter()
                            .filter(|(_, entries)| !entries.is_empty())
                            .map(|(event, entries)| LoadedHookContribution {
                                item,
                                event,
                                entries,
                            }),
                    ),
                    Err(error) => {
                        diag.error("hook-fragment", error.to_string());
                        continue;
                    }
                }
            }
        }
        let mut target_hooks =
            resolve_hook_collisions_for_target(&contributions, target_root, diag);
        target_hooks.sort_by_key(|hook| {
            (
                hook.item.package_depth,
                hook.item.decl_order,
                hook.item.def.order,
                hook.item.def.name.as_str(),
                hook.event.as_str(),
            )
        });
        let hook_entries = target_hooks.into_iter().map(|hook| {
            ConfigEntry::Hook(HookEntry {
                name: hook.item.def.name.clone(),
                native_event: hook.event.clone(),
                entries: hook.entries.clone(),
            })
        });

        // Combine all entries.
        entries.extend(hook_entries);

        // Resolve and load whole-file hooks independently of merge ordering.
        let mut file_writes = Vec::new();
        if mode == Some(crate::target::HookFragmentMode::File) {
            for item in resolve_file_hook_collisions_for_target(&all_hooks, target_root, diag) {
                let installed = ctx
                    .project_root
                    .join(target_root)
                    .join("hooks")
                    .join(&item.def.name);
                if !dry_run && !hook_directory_is_installed(item, &installed) {
                    diag.warn(
                        "hook-not-installed",
                        format!(
                            "not emitting hook `{}` for `{target_root}` because `{}` is not installed",
                            item.def.name,
                            installed.display()
                        ),
                    );
                    continue;
                }
                let Some(relative_dest) = adapter.hook_file_dest_path(&item.def.name) else {
                    diag.error(
                        "hook-fragment",
                        format!(
                            "target `{target_root}` declares file-mode hooks without a placement path"
                        ),
                    );
                    continue;
                };
                match crate::compiler::hooks::load_file_fragment(item, target_root, &installed) {
                    Ok(content) => {
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
            target_records.insert(
                entry.key(),
                ConfigEntryRecord {
                    emitted_json: match entry {
                        ConfigEntry::Hook(hook) => serde_json::to_string(&hook.entries).ok(),
                        ConfigEntry::McpServer(_) => None,
                    },
                },
            );
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

fn source_may_emit_mcp(graph: &crate::resolve::ResolvedGraph, source_name: &SourceName) -> bool {
    use crate::config::FilterMode;

    graph.filters.get(source_name).is_none_or(|filters| {
        filters
            .iter()
            .any(|filter| matches!(filter, FilterMode::All | FilterMode::Exclude(_)))
    })
}

fn hook_directory_is_installed(
    item: &crate::compiler::hooks::ParsedHookItem,
    installed: &std::path::Path,
) -> bool {
    crate::hash::compute_hash(installed, crate::lock::ItemKind::Hook)
        .ok()
        .zip(crate::hash::compute_hash(&item.hook_dir, crate::lock::ItemKind::Hook).ok())
        .is_some_and(|(installed, desired)| installed == desired)
}

/// Compute package depth for hook ordering.
///
/// Direct dependencies of the consumer project have depth 1.
/// Their transitive dependencies have depth 2, etc.
/// Packages at the leaf of the graph (no dependencies themselves) have the highest depth.
///
/// Returns a map from SourceName → depth.
fn compute_depths(graph: &crate::resolve::ResolvedGraph) -> HashMap<SourceName, usize> {
    // A dependency is ready only after every package that depends on it has
    // contributed its depth. This is Kahn's topological algorithm over the
    // reversed dependency edges, with graph.order as the deterministic tie
    // breaker for direct dependencies.
    let mut remaining_dependents: HashMap<SourceName, usize> = HashMap::new();
    for name in &graph.order {
        remaining_dependents.insert(name.clone(), 0);
    }
    for name in &graph.order {
        let Some(node) = graph.nodes.get(name) else {
            continue;
        };
        for dep in &node.deps {
            if graph.nodes.contains_key(dep) {
                *remaining_dependents.entry(dep.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut depths: HashMap<SourceName, usize> = HashMap::new();
    let mut queue: VecDeque<SourceName> = VecDeque::new();
    for name in &graph.order {
        if remaining_dependents.get(name) == Some(&0) {
            depths.insert(name.clone(), 1);
            queue.push_back(name.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        let current_depth = depths[&current];
        if let Some(node) = graph.nodes.get(&current) {
            for dep in &node.deps {
                let Some(remaining) = remaining_dependents.get_mut(dep) else {
                    continue;
                };
                depths
                    .entry(dep.clone())
                    .and_modify(|depth| *depth = (*depth).max(current_depth + 1))
                    .or_insert(current_depth + 1);
                *remaining -= 1;
                if *remaining == 0 {
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    depths
}

/// Compute declaration-order precedence for dependency config entries.
///
/// Direct dependencies use the insertion order from `effective.dependencies`.
/// Transitive dependencies inherit the minimum declaration order of any direct
/// sponsor that reaches them.
fn compute_decl_orders(
    graph: &crate::resolve::ResolvedGraph,
    dependencies: &indexmap::IndexMap<SourceName, crate::config::EffectiveDependency>,
) -> HashMap<SourceName, usize> {
    let mut orders: HashMap<SourceName, usize> = HashMap::new();
    let mut queue: VecDeque<SourceName> = VecDeque::new();

    for (idx, source_name) in dependencies.keys().enumerate() {
        if graph.nodes.contains_key(source_name) {
            orders.insert(source_name.clone(), idx + 1);
            queue.push_back(source_name.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        let current_order = orders[&current];
        let Some(node) = graph.nodes.get(&current) else {
            continue;
        };

        for dep in &node.deps {
            if !graph.nodes.contains_key(dep) {
                continue;
            }
            match orders.get_mut(dep) {
                Some(existing) if current_order < *existing => {
                    *existing = current_order;
                    queue.push_back(dep.clone());
                }
                Some(_) => {}
                None => {
                    orders.insert(dep.clone(), current_order);
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    orders
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{ResolvedGraph, ResolvedNode, RootedSourceRef};
    use crate::source::ResolvedRef;
    use crate::types::SourceId;
    use indexmap::IndexMap;
    use std::path::PathBuf;

    fn graph_node(name: &str, deps: &[&str]) -> ResolvedNode {
        let root = PathBuf::from(format!("/tmp/{name}"));
        ResolvedNode {
            source_name: name.into(),
            source_id: SourceId::Path {
                canonical: root.clone(),
                subpath: None,
            },
            rooted_ref: RootedSourceRef {
                checkout_root: root.clone(),
                package_root: root.clone(),
            },
            resolved_ref: ResolvedRef {
                source_name: name.into(),
                version: None,
                version_tag: None,
                commit: None,
                tree_path: root,
            },
            manifest: None,
            deps: deps.iter().map(|dep| (*dep).into()).collect(),
        }
    }

    #[test]
    fn hook_depths_are_stable_for_longer_path_discovered_after_shorter_path() {
        let definitions = [
            ("direct-short", vec!["shared"]),
            ("shared", vec!["leaf"]),
            ("leaf", vec![]),
            ("direct-long", vec!["middle"]),
            ("middle", vec!["shared"]),
        ];
        let graph_order: Vec<SourceName> =
            ["direct-short", "shared", "leaf", "direct-long", "middle"]
                .into_iter()
                .map(SourceName::from)
                .collect();
        let expected_emission = vec![
            "direct-long".to_string(),
            "direct-short".to_string(),
            "middle".to_string(),
            "shared".to_string(),
            "leaf".to_string(),
        ];

        for run in 0..128 {
            let mut insertion_order = definitions.clone();
            let definition_count = insertion_order.len();
            insertion_order.rotate_left(run % definition_count);
            if run % 2 == 1 {
                insertion_order.reverse();
            }
            let nodes = insertion_order
                .into_iter()
                .map(|(name, deps)| (name.into(), graph_node(name, &deps)))
                .collect::<IndexMap<_, _>>();
            let graph = ResolvedGraph {
                order: graph_order.clone(),
                nodes,
                filters: HashMap::new(),
                version_constraints: HashMap::new(),
                unreadable_hook_surfaces: std::collections::BTreeMap::new(),
            };
            let depths = compute_depths(&graph);
            let mut emitted_order = graph.order.clone();
            emitted_order.sort_by_key(|name| (depths[name], name.clone()));
            assert_eq!(
                emitted_order
                    .into_iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
                expected_emission,
            );
        }
    }
}
