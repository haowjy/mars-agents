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
use crate::target::keyed_entries::{self, KeyedKind};
use crate::target::mcp_file::{self, MCP_KIND};
use crate::target::plugin::HOOK_ENTRY_KIND;
use crate::target::{HOOK_KEY_PREFIX, HookEntry, McpDirs, McpServerEntry, TargetRegistry};
use crate::types::MarsContext;
use file_hooks::{file_hook_output_preserve_paths, kept_file_outputs, write_file_hook_outputs};
use mars_schema::hooks::HookOutput;
use render::{RenderedHooks, lower_merge_outputs};

pub(crate) struct ConfigEntryCompilation {
    pub records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    pub emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub rewritten_outputs: Vec<(String, String)>,
    /// Keyed-entry config files this run wrote or deleted, as `(target_root, file path, what
    /// it holds)`.
    pub mcp_files_changed: Vec<(String, String, &'static str)>,
    pub removed_outputs: Vec<(String, String)>,
}

/// The caller owns eligibility checks, including whether an installed MCP
/// directory is current, and supplies each item's directory tokens with it: the
/// replacements for `${MARS_MCP_DIR}` in a server's command and arguments.
pub(crate) fn lower_mcp_servers(mcp: &[(McpDirs, &ParsedMcpItem)]) -> Vec<McpServerEntry> {
    mcp.iter()
        .map(|(dir, parsed)| McpServerEntry {
            name: parsed.name.clone(),
            transport: lower_transport(parsed, dir),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_config_entries(
    ctx: &MarsContext,
    applied: &AppliedState,
    ownership_lock: &mut crate::lock::LockFile,
    target_outcomes: &[crate::target_sync::TargetSyncOutcome],
    rendered_hooks: &RenderedHooks,
    dry_run: bool,
    force: bool,
    preview_warnings: bool,
    diag: &mut DiagnosticCollector,
) -> ConfigEntryCompilation {
    use crate::compiler::mcp::check_env_refs;
    use crate::target::TargetRegistry;

    let effective = &applied.planned.targeted.resolved.loaded.effective;
    let target_roots: Vec<String> = crate::config::targets::managed_targets(&effective.settings);
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
        diag.warn_advisory("mcp-env", format!("MCP env check failed: {e}"));
    }

    let registry = TargetRegistry::new();
    let mut desired_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>> =
        BTreeMap::new();
    let mut desired_keyed: DesiredKeyed = BTreeMap::new();
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
            .map(|(dir, item)| {
                (
                    adapter.mcp_dirs(&dir.native(), &installed_token(ctx, dir)),
                    *item,
                )
            })
            .collect::<Vec<_>>();
        let mcp_servers = lower_mcp_servers(&mcp_entries);

        let mut merge_outputs = Vec::new();
        let mut keyed_hooks = Vec::new();
        let mut file_writes = Vec::new();
        for (dir, item) in &installed_hooks {
            match rendered_hooks.get(dir) {
                HookOutput::Merge(output) => merge_outputs.push((*item, output)),
                HookOutput::Keyed(output) => {
                    keyed_hooks.push(keyed_entries::Rendered {
                        name: item.def.name.clone(),
                        value: output.keyed_value(),
                    });
                    for file in &output.files {
                        desired_file_outputs.insert((target_root.clone(), file.path.clone()));
                        file_writes.push((
                            dir.canonical().as_str().to_string(),
                            file.path.clone(),
                            file.content.clone(),
                        ));
                    }
                }
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
            // A keyed hook is one entry named after the hook; the old record is its value.
            if let Some(record) = old_lock
                .config_entries
                .get(target_root)
                .and_then(|records| records.get(&HOOK_ENTRY_KIND.key(&kept.name)))
            {
                match serde_json::from_str(&record.emitted_json) {
                    Ok(value) => keyed_hooks.push(keyed_entries::Rendered {
                        name: kept.name.clone(),
                        value,
                    }),
                    Err(error) => diag.warn(
                        "hook-invalid-kept",
                        format!("cannot keep hook `{}` for `{target_root}`: its record is unreadable ({error})", kept.name),
                    ),
                }
            }
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

        let hooks = lower_merge_outputs(&merge_outputs, kept_entries);

        // Render each server once. Lossiness warnings come with the render, so
        // they run even on dry runs.
        let rendered = adapter
            .mcp_file()
            .map(|file| mcp_file::render_all(&file, &mcp_servers, diag))
            .unwrap_or_default();
        let mut rendered = rendered;
        if applied.planned.targeted.target.mcp_scan_failed() {
            // Which source a recorded server came from is not recorded, so every server the
            // last sync installed and this one did not select stays.
            for (key, record) in old_lock
                .config_entries
                .get(target_root)
                .into_iter()
                .flatten()
            {
                let Some(name) = key.strip_prefix(MCP_KIND.key_prefix) else {
                    continue;
                };
                if rendered.iter().all(|server| server.key(&MCP_KIND) != *key)
                    && let Ok(value) = serde_json::from_str(&record.emitted_json)
                {
                    rendered.push(keyed_entries::Rendered {
                        name: name.to_string(),
                        value,
                    });
                }
            }
        }
        let keyed: Vec<(KeyedKind, keyed_entries::Rendered)> = rendered
            .into_iter()
            .map(|server| (MCP_KIND, server))
            .chain(adapter.fixed_entries())
            .chain(keyed_hooks.into_iter().map(|hook| (HOOK_ENTRY_KIND, hook)))
            .collect();
        let target_records: BTreeMap<_, _> = keyed
            .iter()
            .map(|(kind, entry)| (entry.key(kind), entry.record()))
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
        if !keyed.is_empty() {
            desired_keyed.insert(target_root.clone(), keyed);
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
                diag.warn_advisory(
                    "stale-config-entry",
                    format!(
                        "target `{target_root}` has stale config entries: {} — run `{}` to remove them",
                        stale.join(", "),
                        crate::types::managed_cmd("mars sync"),
                    ),
                );
            }
        }
        let mut rewritten_outputs: Vec<(String, String)> = pending_file_writes
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
        // A merged hook file that is not already as a real sync would leave it is rewritten.
        rewritten_outputs.extend(pending_hooks.keys().filter_map(|target_root| {
            merge_file(target_root).map(|merge| (target_root.clone(), merge.file.to_string()))
        }));
        // Preview the MCP files a real sync would write. A dry run has no real pass after
        // this one, so its warnings are the ones `--diff` shows; other dry runs drop them.
        let mut scratch = DiagnosticCollector::new();
        let preview_diag = if preview_warnings {
            &mut *diag
        } else {
            &mut scratch
        };
        let mcp_files_changed = reconcile_all_keyed(
            &registry,
            &ctx.project_root,
            previous_records,
            &desired_keyed,
            force,
            true,
            preview_diag,
        )
        .files_changed;
        return ConfigEntryCompilation {
            records: desired_records,
            emitted_outputs,
            rewritten_outputs,
            mcp_files_changed,
            removed_outputs,
        };
    }

    // Codex skips a hook it has not been told to trust, keyed on its position in the file;
    // `hooks.json` before this sync is what the user last reviewed.
    let trust_before = trust_snapshots(ctx, &target_roots);

    let to_remove: BTreeMap<_, _> = previous_records
        .iter()
        .filter(|(target_root, _)| !unchanged_records.contains_key(*target_root))
        .map(|(target_root, records)| (target_root.clone(), records.clone()))
        .collect();
    let retention =
        crate::surface_ownership::retention::RemovalPlan::build(&to_remove, &desired_records)
            .execute(
                |operation, diag| {
                    // A target that gets a write is edited there, removal and append in one
                    // load-and-save; removing it here too would save the file twice.
                    if pending_hooks.contains_key(operation.target_root()) {
                        return RemovalReport::confirmed();
                    }
                    remove_hooks(operation, &ctx.project_root, diag)
                },
                diag,
            );

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

    let KeyedReconciliation {
        records: keyed_records,
        removed: keyed_removed,
        files_changed: mcp_files_changed,
    } = reconcile_all_keyed(
        &registry,
        &ctx.project_root,
        previous_records,
        &desired_keyed,
        force,
        false,
        diag,
    );
    let mut compiled = apply_replacement_writes(
        ctx,
        old_lock,
        ownership_lock,
        retention,
        pending_hooks,
        &to_remove,
        &keyed_removed,
        pending_file_writes,
        &desired_records,
        unchanged_records,
        force,
        diag,
        emitted_outputs,
        removed_outputs,
    );
    compiled.mcp_files_changed = mcp_files_changed;
    for (target_root, records) in keyed_records {
        compiled
            .records
            .entry(target_root)
            .or_default()
            .extend(records);
    }
    warn_untrusted(&trust_before, &trust_snapshots(ctx, &target_roots), diag);
    compiled
}

type TrustSnapshots = BTreeMap<String, (String, crate::target::hook_file::TrustPositions)>;

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

fn warn_untrusted(before: &TrustSnapshots, after: &TrustSnapshots, diag: &mut DiagnosticCollector) {
    for (root, (file, positions)) in after {
        let Some((_, previous)) = before.get(root) else {
            continue;
        };
        let changed = crate::target::hook_file::untrusted_positions(previous, positions);
        if changed.is_empty() {
            continue;
        }
        diag.warn_advisory(
            "codex-hooks-need-trust",
            format!(
                "`{file}` has {} new or moved hook handler(s) ({}); open Codex and run `/hooks` to review and trust them. Codex skips them until then",
                changed.len(),
                changed.join(", ")
            ),
        );
    }
}

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
        None => RemovalReport::confirmed(),
    };
    report.context(format!(
        "failed to remove prior hook entries from `{target_root}`"
    ))
}

pub(crate) fn report_removed(
    hooks: &BTreeMap<String, Vec<String>>,
    keyed: &BTreeMap<String, Vec<String>>,
    diag: &mut DiagnosticCollector,
) {
    for target_root in hooks.keys().chain(keyed.keys()).collect::<BTreeSet<_>>() {
        let keys: Vec<&str> = [hooks.get(target_root), keyed.get(target_root)]
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

/// What each target wants in its keyed files, by target root.
type DesiredKeyed = BTreeMap<String, Vec<(KeyedKind, keyed_entries::Rendered)>>;

pub(crate) struct KeyedReconciliation {
    pub records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    pub removed: BTreeMap<String, Vec<String>>,
    /// `(target_root, file path, what the file holds)` of each file written or deleted (or, in
    /// a dry run, to be).
    pub files_changed: Vec<(String, String, &'static str)>,
}

/// Reconcile every keyed section each target's adapter declares: a target no longer managed
/// still gets its recorded entries removed.
pub(crate) fn reconcile_all_keyed(
    registry: &TargetRegistry,
    project_root: &Path,
    previous_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    desired: &DesiredKeyed,
    force: bool,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> KeyedReconciliation {
    let mut out = KeyedReconciliation {
        records: BTreeMap::new(),
        removed: BTreeMap::new(),
        files_changed: Vec::new(),
    };
    let no_records = BTreeMap::new();
    for target_root in previous_records
        .keys()
        .chain(desired.keys())
        .collect::<BTreeSet<_>>()
    {
        let Some(adapter) = registry.get(target_root) else {
            continue;
        };
        let previous = previous_records.get(target_root).unwrap_or(&no_records);
        for (kind, section) in adapter.keyed_sections() {
            let wanted: Vec<_> = desired
                .get(target_root)
                .into_iter()
                .flatten()
                .filter(|(wanted_kind, _)| *wanted_kind == kind)
                .map(|(_, entry)| entry.clone())
                .collect();
            let reconciled = keyed_entries::reconcile(
                &kind,
                &section,
                project_root,
                target_root,
                previous,
                &wanted,
                force,
                dry_run,
                diag,
            );
            if !reconciled.records.is_empty() {
                out.records
                    .entry(target_root.clone())
                    .or_default()
                    .extend(reconciled.records);
            }
            out.removed
                .entry(target_root.clone())
                .or_default()
                .extend(reconciled.removed);
            if reconciled.changed {
                out.files_changed.push((
                    target_root.clone(),
                    section.path.to_string(),
                    kind.plural,
                ));
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn apply_replacement_writes(
    ctx: &MarsContext,
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    retention: crate::surface_ownership::retention::RetentionPlan,
    pending_hooks: BTreeMap<String, Vec<HookEntry>>,
    prior_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    keyed_removed: &BTreeMap<String, Vec<String>>,
    pending_file_writes: BTreeMap<String, Vec<(String, String, String)>>,
    desired_records: &BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    unchanged_records: BTreeMap<String, BTreeMap<String, ConfigEntryRecord>>,
    force: bool,
    diag: &mut DiagnosticCollector,
    mut emitted_outputs: Vec<crate::lock::CompiledNativeOutput>,
    removed_outputs: Vec<(String, String)>,
) -> ConfigEntryCompilation {
    // A target's recorded hooks are removed and the desired ones appended in `hook_file::replace`,
    // after keyed-entry reconcile, so the file is loaded after every other writer of it is done. A failed
    // write leaves the file as it was, so the target keeps its prior records.
    let mut written_records = unchanged_records;
    let mut failed_targets = BTreeSet::new();
    let mut rewritten_outputs = Vec::new();
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
        let prior = prior_records.get(&target_root);
        if let Err(error) = crate::target::hook_file::replace(
            &target_dir.join(merge.file),
            prior.unwrap_or(&BTreeMap::new()),
            &target_root,
            &hooks,
            merge.top_level,
            diag,
        ) {
            diag.warn(
                "config-entry-write",
                format!("failed to write config entries to `{target_root}`: {error}"),
            );
            written_records
                .entry(target_root.clone())
                .or_default()
                .extend(
                    prior
                        .into_iter()
                        .flatten()
                        .filter(|(key, _)| key.starts_with(HOOK_KEY_PREFIX))
                        .map(|(key, record)| (key.clone(), record.clone())),
                );
            failed_targets.insert(target_root);
            continue;
        }
        rewritten_outputs.push((target_root.clone(), merge.file.to_string()));
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
            &mut rewritten_outputs,
        );
    }

    let mut removed_hooks = retention.removed_keys().clone();
    removed_hooks.retain(|target_root, _| !failed_targets.contains(target_root));
    report_removed(&removed_hooks, keyed_removed, diag);

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
        mcp_files_changed: Vec::new(),
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

fn merge_file(target_root: &str) -> Option<&'static mars_schema::hooks::targets::MergeFile> {
    mars_schema::hooks::targets::target(target_root)?.merge_file()
}

fn installed_path(ctx: &MarsContext, dir: &PackageDir) -> PathBuf {
    ctx.project_root.join(dir.target()).join(dir.native())
}

fn installed_token(ctx: &MarsContext, dir: &PackageDir) -> String {
    installed_path(ctx, dir)
        .to_string_lossy()
        .replace('\\', "/")
}
