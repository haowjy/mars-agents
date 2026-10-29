//! Config-entry compiler lane for MCP servers and hooks.
//!
//! This module owns config-entry lowering and writes. MCP and hook selection
//! come from the target build that also selects installed content.

pub(crate) mod file_hooks;
pub(crate) mod render;
pub mod resolve;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::compiler::mcp::{ParsedMcpItem, lower_transport};
use crate::diagnostic::DiagnosticCollector;
use crate::lock::ConfigEntryRecord;
use crate::surface_ownership::retention::{RemovalOperation, RemovalReport};
use crate::sync::AppliedState;
use crate::sync::package_dir::PackageDir;
use crate::target::{
    HOOK_KEY_PREFIX, HookEntry, MCP_KEY_PREFIX, McpServerEntry, TargetRegistry, mcp_file,
};
use crate::types::MarsContext;
use file_hooks::{file_hook_output_preserve_paths, kept_file_outputs, write_file_hook_outputs};
use mars_schema::hooks::HookOutput;
use render::{RenderedHooks, lower_merge_native_hooks};

pub(crate) struct ConfigEntryCompilation {
    pub records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    pub emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    /// Generated hook files whose bytes this run wrote, as `(target_root, dest_path)`.
    pub rewritten_outputs: Vec<(String, String)>,
    pub removed_outputs: Vec<(String, String)>,
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_config_entries(
    ctx: &MarsContext,
    applied: &AppliedState,
    ownership_lock: &mut crate::lock::LockFile,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
    rendered_hooks: &RenderedHooks,
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
        let mcp_entries = selected_mcp
            .iter()
            .map(|(dir, item)| (installed_token(ctx, dir), *item))
            .collect::<Vec<_>>();
        let mcp_servers = lower_mcp_servers(&mcp_entries);

        // Preflight rendered every selected hook; place each by its target's placement.
        let mut merge_native_hooks = Vec::new();
        let mut file_writes = Vec::new();
        for (dir, item) in &installed_hooks {
            match rendered_hooks.get(dir) {
                HookOutput::Merge(native_hook) => merge_native_hooks.push((*item, native_hook)),
                HookOutput::File(output) => {
                    for file in &output.files {
                        desired_file_outputs.insert((target_root.clone(), file.path.clone()));
                        file_writes.push((
                            dir.canonical().as_str().to_string(),
                            file.path.clone(),
                            file.content.clone(),
                        ));
                    }
                }
            }
        }

        // A kept hook (see `sync::target::KeptHook`) stays as the last lock recorded it.
        let mut kept_records = BTreeMap::new();
        let mut kept_entries = Vec::new();
        for kept in applied.planned.targeted.target.kept_hooks() {
            let Some(dir) = kept.dirs.iter().find(|dir| dir.belongs_to(target_root)) else {
                continue;
            };
            desired_file_outputs.extend(
                kept_file_outputs(old_lock, dir)
                    .into_iter()
                    .map(|path| (target_root.clone(), path)),
            );
            for (key, record) in old_lock
                .config_entries
                .get(target_root)
                .into_iter()
                .flatten()
            {
                let Some((event, name)) = key
                    .strip_prefix(HOOK_KEY_PREFIX)
                    .and_then(|rest| rest.split_once(':'))
                    .filter(|(_, name)| *name == kept.name)
                else {
                    continue;
                };
                match serde_json::from_str(&record.emitted_json) {
                    Ok(entries) => {
                        kept_entries.push((
                            kept.rank,
                            HookEntry {
                                name: name.to_string(),
                                native_event: event.to_string(),
                                entries,
                            },
                        ));
                        kept_records.insert(key.clone(), record.clone());
                    }
                    Err(error) => diag.warn(
                        "hook-invalid-kept",
                        format!("cannot keep `{key}` for `{target_root}`: its record is unreadable ({error})"),
                    ),
                }
            }
        }

        let hooks = lower_merge_native_hooks(&merge_native_hooks, kept_entries);

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
                let key = hook.key();
                let record = kept_records
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| ConfigEntryRecord {
                        emitted_json: serde_json::to_string(&hook.entries)
                            .expect("JSON values serialize"),
                    });
                (key, record)
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
        // Preview the generated hook files a real sync would write: a missing file or one
        // whose bytes differ from what would be rendered.
        let rewritten_outputs = pending_file_writes
            .iter()
            .flat_map(|(target_root, files)| {
                files.iter().filter_map(move |(_, relative, content)| {
                    let path = ctx.project_root.join(target_root).join(relative);
                    let current = std::fs::read(path).ok();
                    (current.as_deref() != Some(content.as_bytes()))
                        .then(|| (target_root.clone(), relative.clone()))
                })
            })
            .collect();
        return ConfigEntryCompilation {
            records: desired_records,
            emitted_outputs,
            rewritten_outputs,
            removed_outputs,
        };
    }

    // Codex skips a hook it has not been told to trust, keyed on its position in the file;
    // `hooks.json` before this sync is what the user last reviewed.
    let trust_before = trust_snapshots(ctx, &target_roots);

    // A target whose hook entries equal what the lock records, and whose file still holds
    // them in the order they would be written, is left alone: removing and re-appending them
    // would rewrite the file and move entries (Codex keys its trust on entry position). A file
    // that lost or edited an entry, or whose `order` changed, goes through removal, which
    // warns of the divergence and appends the package entries in order.
    let hook_records = |records: Option<&BTreeMap<String, ConfigEntryRecord>>| {
        records
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.starts_with(HOOK_KEY_PREFIX))
            .map(|(key, record)| (key.clone(), record.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let mut unchanged_records = BTreeMap::new();
    for target_root in pending_hooks.keys().cloned().collect::<Vec<_>>() {
        let desired = hook_records(desired_records.get(&target_root));
        let previous = hook_records(previous_records.get(&target_root));
        let holds_previous = desired == previous
            && merge_file(&target_root).is_some_and(|merge| {
                crate::target::hook_file::holds_recorded(
                    &ctx.project_root.join(&target_root).join(merge.file),
                    &pending_hooks[&target_root],
                )
            });
        if holds_previous {
            pending_hooks.remove(&target_root);
            unchanged_records.insert(target_root, desired);
        }
    }
    let to_remove: BTreeMap<_, _> = previous_records
        .iter()
        .filter(|(target_root, _)| !unchanged_records.contains_key(*target_root))
        .map(|(target_root, records)| (target_root.clone(), records.clone()))
        .collect();
    let retention =
        crate::surface_ownership::retention::RemovalPlan::build(&to_remove, &desired_records)
            .execute(
                |operation, diag| remove_hooks(operation, &ctx.project_root, diag),
                diag,
            );

    // File-mode native hooks are ordinary target outputs. Remove only exact paths
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
        old_lock,
        ownership_lock,
        retention,
        pending_hooks,
        pending_file_writes,
        &desired_records,
        unchanged_records,
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
    warn_untrusted(&trust_before, &trust_snapshots(ctx, &target_roots), diag);
    compiled
}

/// Per trust-gated target root: its hook file's display path and handler positions.
type TrustSnapshots = BTreeMap<String, (String, crate::target::hook_file::TrustPositions)>;

/// The handler positions of each managed target whose harness gates hooks on trust.
fn trust_snapshots(ctx: &MarsContext, target_roots: &[String]) -> TrustSnapshots {
    target_roots
        .iter()
        .filter_map(|root| {
            let target = mars_schema::hooks::targets::target(root)?;
            let file = target.merge_file().filter(|m| m.trust_gated)?.file;
            let positions = crate::target::hook_file::trust_positions(
                &ctx.project_root.join(root).join(file),
                |event| target.reads(event),
            );
            Some((root.clone(), (format!("{root}/{file}"), positions)))
        })
        .collect()
}

/// Say which hooks the harness will skip until the user trusts them again (D10).
fn warn_untrusted(before: &TrustSnapshots, after: &TrustSnapshots, diag: &mut DiagnosticCollector) {
    for (root, (file, positions)) in after {
        let Some((_, previous)) = before.get(root) else {
            continue;
        };
        let changed = crate::target::hook_file::untrusted_positions(previous, positions);
        if changed.is_empty() {
            continue;
        }
        diag.warn(
            "codex-hooks-need-trust",
            format!(
                "`{file}` has {} new or moved hook handler(s) ({}); `{root}` skips them until you review them in its `/hooks` screen",
                changed.len(),
                changed.join(", ")
            ),
        );
    }
}

/// Remove the hook entries `operation` names. Shared by sync and `mars unlink`.
pub(crate) fn remove_hooks(
    operation: RemovalOperation<'_>,
    project_root: &Path,
    diag: &mut DiagnosticCollector,
) -> RemovalReport {
    let target_root = operation.target_root().to_owned();
    let (target_dir, removal) = operation.into_parts(project_root);
    let report = match merge_file(&target_root) {
        Some(merge) => crate::target::hook_file::remove_recorded(
            &target_dir.join(merge.file),
            &removal.prior_records,
            &target_root,
            diag,
        )
        .map_or_else(
            |error| RemovalReport::failed(error, removal.prior_records.clone()),
            |()| RemovalReport::confirmed(),
        ),
        // A file target records no merge entries.
        None => RemovalReport::confirmed(),
    };
    report.context(format!(
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
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    retention: crate::surface_ownership::retention::RetentionPlan,
    pending_hooks: BTreeMap<String, Vec<HookEntry>>,
    pending_file_writes: BTreeMap<String, Vec<(String, String, String)>>,
    desired_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    unchanged_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    force: bool,
    diag: &mut DiagnosticCollector,
    mut emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    removed_outputs: Vec<(String, String)>,
) -> ConfigEntryCompilation {
    // Write desired hooks only after every removal has completed. Removal
    // cleared the target's recorded hooks, so a failed write leaves no record.
    let mut written_records = unchanged_records;
    for (target_root, hooks) in pending_hooks {
        let Some(permit) = retention.write_permit(&target_root) else {
            diag.info(
                "config-entry-suppressed",
                format!("not writing Hook entries to `{target_root}`: prior removal unconfirmed"),
            );
            continue;
        };
        let write = permit.bind_hooks(hooks);
        let keys: Vec<String> = write.hooks().iter().map(HookEntry::key).collect();
        let Some(merge) = merge_file(write.target_root()) else {
            continue;
        };
        let (target_dir, hooks) = write.into_parts(&ctx.project_root);
        if let Err(error) =
            crate::target::hook_file::write(&target_dir.join(merge.file), &hooks, merge.top_level)
        {
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

    let mut rewritten_outputs = Vec::new();
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
            &mut rewritten_outputs,
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
        rewritten_outputs,
        removed_outputs,
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

/// The hook config file a merge target appends entries to; `None` for a file target and for
/// a root with no hook mechanism.
fn merge_file(target_root: &str) -> Option<&'static mars_schema::hooks::targets::MergeFile> {
    mars_schema::hooks::targets::target(target_root)?.merge_file()
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
