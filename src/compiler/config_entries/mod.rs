//! Config-entry compiler lane for MCP servers and hooks.
//!
//! This module owns discovery, filtering, lowering, and target-adapter writes
//! for package-defined MCP servers and hooks.

pub mod resolve;
pub mod stale;

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::sync::AppliedState;
use crate::types::{MarsContext, SourceName};

/// Validate all hook schemas and native event names before the apply phase.
///
/// This is deliberately separate from config emission so an invalid hook
/// cannot leave canonical or target state partially mutated.
pub(crate) fn preflight_hooks(
    ctx: &MarsContext,
    resolved: &crate::sync::ResolvedState,
    diag: &mut DiagnosticCollector,
) -> Result<(), crate::error::MarsError> {
    use crate::compiler::hooks::{discover_hook_items, load_file_fragment, load_merge_fragment};
    use crate::error::{ConfigError, MarsError};
    use crate::target::{HookFragmentMode, TargetRegistry};

    let mut hooks = discover_hook_items(&ctx.project_root, "_self", 0, 0)?;
    for (decl_order, source_name) in resolved.graph.order.iter().enumerate() {
        if let Some(node) = resolved.graph.nodes.get(source_name) {
            hooks.extend(discover_hook_items(
                &node.rooted_ref.package_root,
                source_name.as_str(),
                1,
                decl_order,
            )?);
        }
    }
    let registry = TargetRegistry::new();
    let mut errors = Vec::new();
    for item in hooks {
        for target_name in item.def.targets.keys() {
            let adapter = registry.get(target_name);
            let mode = adapter.and_then(|adapter| adapter.hook_fragment_mode());
            let installed = ctx
                .project_root
                .join(target_name)
                .join("hooks")
                .join(&item.def.name);
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

/// Phase 5 config-entry compilation: MCP servers and hooks.
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
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> {
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

    // Compute package depths via BFS from direct deps (depth 1; local = 0).
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

        match discover_mcp_items(package_root, source_name.as_str(), decl_order) {
            Ok(items) => all_mcp.extend(items),
            Err(e) => {
                diag.warn(
                    "mcp-discover",
                    format!("failed to scan MCP items in `{source_name}`: {e}"),
                );
            }
        }

        let depth = depths.get(source_name).copied().unwrap_or(1);
        match discover_hook_items(package_root, source_name.as_str(), depth, decl_order) {
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
    let mut pending_writes: BTreeMap<String, Vec<ConfigEntry>> = BTreeMap::new();
    let mut pending_file_writes: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();

    // First lower every target without mutating its config. Stale hook removal
    // is name-based across native events, so it must run before replacement
    // bindings with the same name are written.
    for target_root in &target_roots {
        // Lower MCP items for this target.
        let mut entries_with_source: Vec<(ConfigEntry, String)> = Vec::new();

        for parsed in resolve_mcp_collisions_for_target(&all_mcp, target_root, diag) {
            let source = parsed.source_name.clone();
            let e = TargetMcpEntry::from_parsed(parsed);
            entries_with_source.push((
                ConfigEntry::McpServer(McpServerEntry {
                    name: e.name,
                    command: e.command,
                    args: e.args,
                    env: e.env.into_iter().collect(),
                }),
                source,
            ));
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
                match crate::compiler::hooks::load_merge_fragment(
                    item,
                    target_root,
                    known_events,
                    &installed,
                    diag,
                    false,
                ) {
                    Ok(fragment) => {
                        contributions.extend(fragment.events.into_iter().map(|(event, entries)| {
                            LoadedHookContribution {
                                item,
                                event,
                                entries,
                            }
                        }))
                    }
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
        let hook_entries: Vec<(ConfigEntry, String)> = target_hooks
            .into_iter()
            .map(|hook| {
                (
                    ConfigEntry::Hook(HookEntry {
                        name: hook.item.def.name.clone(),
                        native_event: hook.event.clone(),
                        entries: hook.entries.clone(),
                    }),
                    hook.item.source_name.clone(),
                )
            })
            .collect();

        // Combine all entries.
        entries_with_source.extend(hook_entries);

        // Resolve and load whole-file hooks independently of merge ordering.
        let mut file_writes = Vec::new();
        let mut file_records = BTreeMap::new();
        if mode == Some(crate::target::HookFragmentMode::File) {
            for item in resolve_file_hook_collisions_for_target(&all_hooks, target_root, diag) {
                let installed = ctx
                    .project_root
                    .join(target_root)
                    .join("hooks")
                    .join(&item.def.name);
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
                        let record_key = format!("hook-file:{}", item.def.name);
                        file_records.insert(
                            record_key.clone(),
                            ConfigEntryRecord {
                                source: item.source_name.clone(),
                                emitted_json: None,
                            },
                        );
                        file_writes.push((
                            record_key,
                            relative_dest.to_string_lossy().into_owned(),
                            content,
                        ));
                    }
                    Err(error) => diag.error("hook-fragment", error.to_string()),
                }
            }
        }

        // Write via the target adapter (if one is registered).
        let entries: Vec<ConfigEntry> = entries_with_source
            .iter()
            .map(|(entry, _)| entry.clone())
            .collect();

        let mut target_records = BTreeMap::new();
        for (entry, source) in &entries_with_source {
            target_records.insert(
                entry.key(),
                ConfigEntryRecord {
                    source: source.clone(),
                    emitted_json: match entry {
                        ConfigEntry::Hook(hook) => serde_json::to_string(&hook.entries).ok(),
                        ConfigEntry::McpServer(_) => None,
                    },
                },
            );
        }

        // Emit target-specific pre-write diagnostics (runs even on dry runs).
        adapter.emit_pre_write_diagnostics(&entries, diag);
        target_records.extend(file_records);
        if !target_records.is_empty() {
            desired_records.insert(target_root.clone(), target_records);
        }
        if !entries.is_empty() {
            pending_writes.insert(target_root.clone(), entries);
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
    let stale_entries = stale::find_stale_entries(previous_records, &desired_records);
    let mut retained_stale_records = BTreeMap::new();

    // Sweep every prior hook emission before writing replacements. Exact fragment
    // arrays come from the lock; records from v0.11.0 intentionally fall back to
    // the one-release command-path bridge in the adapters.
    if !dry_run {
        for (target_root, records) in previous_records {
            if let Some(adapter) = registry.get(target_root) {
                for key in records.keys().filter(|key| key.starts_with("hook-file:")) {
                    let Some(name) = key.strip_prefix("hook-file:") else {
                        continue;
                    };
                    if let Some(relative) = adapter.hook_file_dest_path(name) {
                        let path = ctx.project_root.join(target_root).join(relative);
                        if let Err(error) = std::fs::remove_file(&path)
                            && error.kind() != std::io::ErrorKind::NotFound
                        {
                            diag.warn(
                                "config-entry-remove",
                                format!("failed to remove `{}`: {error}", path.display()),
                            );
                        }
                    }
                }
            }
            let hook_records: BTreeMap<_, _> = records
                .iter()
                .filter(|(key, _)| key.starts_with("hook:"))
                .map(|(key, record)| (key.clone(), record.clone()))
                .collect();
            if hook_records.is_empty() {
                continue;
            }
            if let Some(adapter) = registry.get(target_root) {
                let target_dir = ctx.project_root.join(target_root);
                if let Err(error) = adapter.remove_owned_hook_entries(&hook_records, &target_dir) {
                    diag.warn(
                        "config-entry-remove",
                        format!(
                            "failed to remove prior hook entries from `{target_root}`: {error}"
                        ),
                    );
                }
            }
        }
    }
    for (target_root, keys) in stale_entries {
        if dry_run {
            diag.warn(
                "stale-config-entry",
                format!(
                    "target `{target_root}` has stale config entries: {}",
                    keys.join(", ")
                ),
            );
            continue;
        }

        let Some(adapter) = registry.get(&target_root) else {
            continue;
        };
        let target_dir = ctx.project_root.join(&target_root);
        let non_hook_keys: Vec<String> = keys
            .iter()
            .filter(|key| !key.starts_with("hook:") && !key.starts_with("hook-file:"))
            .cloned()
            .collect();
        if let Err(e) = adapter.remove_config_entries(&non_hook_keys, &target_dir) {
            diag.warn(
                "config-entry-remove",
                format!("failed to remove stale config entries from `{target_root}`: {e}"),
            );
            if let Some(previous_target_records) = previous_records.get(&target_root) {
                let target_records = retained_stale_records
                    .entry(target_root.clone())
                    .or_insert_with(BTreeMap::new);
                for key in &keys {
                    if let Some(record) = previous_target_records.get(key) {
                        target_records.insert(key.clone(), record.clone());
                    }
                }
            }
        } else {
            diag.info(
                "stale-config-entry",
                format!(
                    "removed stale config entries from `{target_root}`: {}",
                    keys.join(", ")
                ),
            );
        }
    }

    if dry_run {
        return desired_records;
    }

    // Write desired entries only after every stale/name-matched binding has
    // been swept. A single sync therefore converges both config and lock when
    // a hook keeps its name but changes from a universal to native event key.
    let mut current_records = retained_stale_records;
    for (target_root, entries) in pending_writes {
        let Some(adapter) = registry.get(&target_root) else {
            continue;
        };
        let target_dir = ctx.project_root.join(&target_root);
        match adapter.write_config_entries(&entries, &target_dir) {
            Ok(_) => {
                if let Some(records) = desired_records.get(&target_root) {
                    current_records
                        .entry(target_root.clone())
                        .or_default()
                        .extend(
                            records
                                .iter()
                                .filter(|(key, _)| !key.starts_with("hook-file:"))
                                .map(|(key, record)| (key.clone(), record.clone())),
                        );
                }
            }
            Err(e) => {
                diag.warn(
                    "config-entry-write",
                    format!("failed to write config entries to `{target_root}`: {e}"),
                );
            }
        }
    }

    for (target_root, files) in pending_file_writes {
        for (record_key, relative, content) in files {
            let path = ctx.project_root.join(&target_root).join(relative);
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
            if let Some(record) = desired_records
                .get(&target_root)
                .and_then(|records| records.get(&record_key))
            {
                current_records
                    .entry(target_root.clone())
                    .or_default()
                    .insert(record_key, record.clone());
            }
        }
    }

    current_records
}

/// Compute package depth for hook ordering.
///
/// Direct dependencies of the consumer project have depth 1.
/// Their transitive dependencies have depth 2, etc.
/// Packages at the leaf of the graph (no dependencies themselves) have the highest depth.
///
/// Returns a map from SourceName → depth.
fn compute_depths(graph: &crate::resolve::ResolvedGraph) -> HashMap<SourceName, usize> {
    // Build reverse adjacency: for each package, which packages it's a dep of.
    // We want BFS from "packages with no inbound edges" — those that no other package depends on.
    // Actually we want the opposite: BFS from packages that nobody else depends on (the leafs),
    // assigning them the highest depth, and the "root" deps get the lowest depth.

    // Simpler: compute depth as the length of the longest path from the package to a leaf.
    // But that's expensive. Use the topological order instead.

    // Approach: packages in graph.order are in topological order (deps first, dependents last).
    // The packages that appear FIRST in topological order (no predecessors) are depth 1.
    // The packages they depend on are depth 2+.
    // Wait, that's also complex.

    // Simplest correct approach: BFS from "packages nobody else depends on" (they are direct deps).
    let mut in_degree: HashMap<SourceName, usize> = HashMap::new();
    for name in graph.nodes.keys() {
        in_degree.insert(name.clone(), 0);
    }
    for node in graph.nodes.values() {
        for dep in &node.deps {
            if graph.nodes.contains_key(dep) {
                *in_degree.entry(dep.clone()).or_insert(0) += 1;
            }
        }
    }

    // Direct dependencies of the consumer project have in_degree 0.
    let mut depths: HashMap<SourceName, usize> = HashMap::new();
    let mut queue: VecDeque<SourceName> = VecDeque::new();

    for (name, degree) in &in_degree {
        if *degree == 0 {
            depths.insert(name.clone(), 1);
            queue.push_back(name.clone());
        }
    }

    // BFS to assign depths to transitives.
    while let Some(current) = queue.pop_front() {
        let current_depth = depths[&current];
        if let Some(node) = graph.nodes.get(&current) {
            for dep in &node.deps {
                if graph.nodes.contains_key(dep) {
                    depths
                        .entry(dep.clone())
                        .and_modify(|d| *d = (*d).max(current_depth + 1))
                        .or_insert_with(|| {
                            queue.push_back(dep.clone());
                            current_depth + 1
                        });
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
