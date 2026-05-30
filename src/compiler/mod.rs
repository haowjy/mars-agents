/// Selective native agent emission via `settings.agent_copy`.
pub mod agent_copy;
/// Compiler stage — target building, diff, plan, apply, lock finalization.
///
/// `compile()` is the second half of the sync pipeline. It consumes a
/// [`crate::model::ReaderIr`] (all source-level facts) and produces a
/// [`crate::sync::SyncReport`] by assigning dest paths, computing diffs,
/// writing files, syncing managed targets, and persisting the lock.
/// Agent-profile schema parser, routing prepass, and per-target lowering.
pub mod agents;
pub mod config_entries;
pub mod context;
/// Hook compiler lane: discovery, event validation, ordering, lossiness classification.
pub mod hooks;
/// MCP server compiler lane: discovery, env-ref validation, collision detection.
pub mod mcp;
/// Skill frontmatter compiler lane: universal schema parsing and native lowering.
pub mod skills;
/// Skill variant layout validation, indexing, and projection helpers.
pub mod variants;
/// Visibility propagation rules for passive vs effectful items (D1/D10).
pub mod visibility;

use std::path::Path;

use indexmap::IndexMap;

use crate::config::AgentEmission;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::model::ReaderIr;
use crate::sync::{
    SyncReport, SyncRequest,
    apply::{ActionOutcome, ActionTaken},
    apply_plan, build_target, check_frozen_gate, create_plan, finalize, sync_targets,
};
use crate::types::MarsContext;

/// Run the compiler stage: `ReaderIr` → target state → plan → apply → `SyncReport`.
pub fn compile(
    ctx: &MarsContext,
    ir: ReaderIr,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<SyncReport, MarsError> {
    // Phase 3: assign dest paths, handle collisions, rewrite frontmatter refs.
    let targeted = build_target(ctx, ir.resolved, ir.local_items, request, diag)?;

    // Phase 4: diff + plan.
    let planned = create_plan(ctx, targeted, request, diag)?;

    // Frozen gate: no pending changes allowed.
    if request.options.frozen {
        check_frozen_gate(&planned)?;
    }

    // Phase 5: persist config mutations, apply plan to canonical store.
    let applied = apply_plan(ctx, planned, request)?;

    // Phase 3.2 / 3.3: Dual-surface compilation — after apply writes agents to
    // .mars/agents/, compile harness-bound agents to their native target directories.
    // Diagnostics run always; file writes are gated on !dry_run.
    let effective_settings = &applied.planned.targeted.resolved.loaded.effective.settings;
    let agent_copy_spec = agent_copy::build_agent_copy_spec(
        effective_settings.agent_copy.as_ref(),
        &effective_settings.managed_targets(),
        diag,
    );
    let agent_surface_policy = agent_surface_policy(
        applied
            .planned
            .targeted
            .resolved
            .loaded
            .config
            .settings
            .agent_emission
            .as_ref(),
        agent_copy_spec.as_ref(),
        ctx.meridian_managed,
    );
    let mars_dir = ctx.project_root.join(".mars");
    let model_aliases = merged_model_aliases_for_native_agents(&applied.planned.targeted.resolved);
    let old_lock = &applied.planned.targeted.resolved.loaded.old_lock;
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: agent_surface_policy.clone(),
            project_root: &ctx.project_root,
            mars_dir: &mars_dir,
            model_aliases: &model_aliases,
            outcomes: &applied.applied.outcomes,
            old_lock,
            dry_run: request.options.dry_run,
        },
        diag,
    );
    let compile_ctx = NativeAgentCompileCtx {
        project_root: &ctx.project_root,
        mars_dir: &mars_dir,
        model_aliases: &model_aliases,
        cursor_probe_slugs: &cached_cursor_probe_slugs_for_native_agents(),
        old_lock,
        options: NativeAgentSurfaceCompileOptions {
            force: request.options.force,
            collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
            dry_run: request.options.dry_run,
        },
    };
    let compiled_native_outputs = match agent_surface_policy {
        AgentSurfacePolicy::EmitAll => dual_surface_compile(&compile_ctx, diag),
        AgentSurfacePolicy::EmitSelective(ref spec) => {
            selective_surface_compile(&compile_ctx, spec, diag)
        }
        AgentSurfacePolicy::SuppressAll => Vec::new(),
    };

    // Phase 5.1 / 5.2 / 5.3: MCP and hooks config-entry compilation.
    // Discovers MCP server and hook items from all packages, validates env refs,
    // detects collisions, and writes per-target config entries via adapters.
    // Diagnostics run always; file writes are gated on !dry_run.
    let config_entry_records =
        config_entries::compile_config_entries(ctx, &applied, request.options.dry_run, diag);

    // Phase 6: copy from canonical store to managed target directories.
    let mut synced = sync_targets(ctx, applied, request, agent_surface_policy, diag);
    synced.config_entries = config_entry_records;
    synced.compiled_native_outputs = compiled_native_outputs;

    // Phase 7: write lock file, build report.
    finalize(ctx, synced, request, diag)
}

/// Describes what happens to agent artifacts on target surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSurfacePolicy {
    /// Emit lowered native agents and copy canonical agents to managed targets.
    EmitAll,
    /// Emit only agents whose model resolves to configured harnesses.
    EmitSelective(agent_copy::AgentCopySpec),
    /// Suppress all agent artifacts on target surfaces.
    SuppressAll,
}

pub fn agent_surface_policy(
    agent_emission: Option<&AgentEmission>,
    agent_copy: Option<&agent_copy::AgentCopySpec>,
    meridian_managed: bool,
) -> AgentSurfacePolicy {
    match agent_emission.unwrap_or(&AgentEmission::Auto) {
        AgentEmission::Always => AgentSurfacePolicy::EmitAll,
        AgentEmission::Auto if !meridian_managed => AgentSurfacePolicy::EmitAll,
        AgentEmission::Auto | AgentEmission::Never => match agent_copy {
            Some(spec) if !spec.harnesses.is_empty() => {
                AgentSurfacePolicy::EmitSelective(spec.clone())
            }
            _ => AgentSurfacePolicy::SuppressAll,
        },
    }
}

/// Convert agent outcomes into removals so target sync can remain a pure
/// materializer with no knowledge of managed-mode policy.
pub fn suppress_agent_outcomes(outcomes: &[ActionOutcome]) -> Vec<ActionOutcome> {
    outcomes
        .iter()
        .cloned()
        .map(|mut outcome| {
            if outcome.item_id.kind == crate::lock::ItemKind::Agent {
                outcome.action = ActionTaken::Removed;
            }
            outcome
        })
        .collect()
}

/// Inputs for native harness agent reconcile (removals outside target sync).
struct NativeAgentReconcileCtx<'a> {
    policy: AgentSurfacePolicy,
    project_root: &'a Path,
    mars_dir: &'a Path,
    model_aliases: &'a IndexMap<String, crate::models::ModelAlias>,
    outcomes: &'a [crate::sync::apply::ActionOutcome],
    old_lock: &'a crate::lock::LockFile,
    dry_run: bool,
}

/// Reconcile native harness agent artifacts written outside target sync.
///
/// Under `SuppressAll`, removes lowered artifacts for all harness-bound agents
/// still present in `.mars/agents/`. Under `EmitSelective`, removes artifacts
/// for agents that no longer qualify. Under `EmitAll`, removes only artifacts
/// for agents removed from the canonical store. Removed agents can no longer be
/// inspected for their previous `harness:`, so removal checks every native
/// harness filename shape.
fn reconcile_native_agent_surfaces(
    ctx: &NativeAgentReconcileCtx<'_>,
    diag: &mut DiagnosticCollector,
) {
    use crate::lock::ItemKind;

    match &ctx.policy {
        AgentSurfacePolicy::SuppressAll => {
            remove_current_native_agent_surfaces(
                ctx.project_root,
                ctx.mars_dir,
                ctx.old_lock,
                ctx.dry_run,
                diag,
            );
        }
        AgentSurfacePolicy::EmitSelective(spec) => {
            reconcile_selective_native_agent_surfaces(
                ctx.project_root,
                ctx.mars_dir,
                spec,
                ctx.model_aliases,
                ctx.old_lock,
                ctx.dry_run,
                diag,
            );
        }
        AgentSurfacePolicy::EmitAll => {}
    }

    for outcome in ctx.outcomes {
        if outcome.item_id.kind != ItemKind::Agent
            || !matches!(outcome.action, ActionTaken::Removed)
        {
            continue;
        }

        let agent_name = outcome.dest_path.item_name(ItemKind::Agent);
        remove_native_agent_shapes(
            ctx.project_root,
            &agent_name,
            ctx.old_lock,
            ctx.dry_run,
            diag,
        );
    }
}

fn remove_current_native_agent_surfaces(
    project_root: &Path,
    mars_dir: &Path,
    old_lock: &crate::lock::LockFile,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) {
    use crate::compiler::agents::parse_agent_content;

    let agents_dir = mars_dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                diag.warn(
                    "native-agent-remove-read",
                    format!("could not read {}: {e}", path.display()),
                );
                continue;
            }
        };

        let mut agent_diags = Vec::new();
        let (profile, _fm) = match parse_agent_content(&content, &mut agent_diags) {
            Ok(r) => r,
            Err(e) => {
                diag.warn(
                    "native-agent-remove-parse",
                    format!("could not parse {}: {e}", path.display()),
                );
                continue;
            }
        };

        let agent_name = profile.name.as_deref().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
        });
        remove_native_agent_shapes(project_root, agent_name, old_lock, dry_run, diag);
    }
}

fn remove_native_agent_shapes(
    project_root: &Path,
    agent_name: &str,
    old_lock: &crate::lock::LockFile,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) {
    use crate::compiler::agents::HarnessKind;

    for harness in HarnessKind::all() {
        let target = harness.target_dir();
        for extension in ["md", "toml"] {
            let dest_rel = format!("agents/{agent_name}.{extension}");
            if !old_lock.contains_output(target, &dest_rel) {
                continue;
            }
            let native_path = project_root
                .join(target)
                .join("agents")
                .join(format!("{agent_name}.{extension}"));
            if !native_path.exists() && native_path.symlink_metadata().is_err() {
                continue;
            }
            if dry_run {
                continue;
            }
            if let Err(e) = crate::reconcile::fs_ops::safe_remove(&native_path) {
                diag.warn(
                    "native-agent-remove",
                    format!("could not remove {}: {e}", native_path.display()),
                );
            }
        }
    }
}

fn reconcile_selective_native_agent_surfaces(
    project_root: &Path,
    mars_dir: &Path,
    spec: &agent_copy::AgentCopySpec,
    model_aliases: &IndexMap<String, crate::models::ModelAlias>,
    old_lock: &crate::lock::LockFile,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) {
    use crate::compiler::agents::{HarnessKind, parse_agent_content};

    let agents_dir = mars_dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                diag.warn(
                    "native-agent-remove-read",
                    format!("could not read {}: {e}", path.display()),
                );
                continue;
            }
        };

        let mut agent_diags = Vec::new();
        let (profile, _) = match parse_agent_content(&content, &mut agent_diags) {
            Ok(r) => r,
            Err(e) => {
                diag.warn(
                    "native-agent-remove-parse",
                    format!("could not parse {}: {e}", path.display()),
                );
                continue;
            }
        };

        let agent_name = profile.name.as_deref().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
        });

        for harness in HarnessKind::all() {
            let qualifies = spec.harnesses.contains(harness)
                && agent_copy::agent_qualifies_for_harness(
                    &profile,
                    harness,
                    model_aliases,
                    spec.include_fanout,
                )
                .is_some();
            if qualifies {
                continue;
            }
            remove_native_agent_shapes_for_harness(
                project_root,
                agent_name,
                harness,
                old_lock,
                dry_run,
                diag,
            );
        }
    }
}

fn remove_native_agent_shapes_for_harness(
    project_root: &Path,
    agent_name: &str,
    harness: &crate::compiler::agents::HarnessKind,
    old_lock: &crate::lock::LockFile,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) {
    let target = harness.target_dir();
    for extension in ["md", "toml"] {
        let dest_rel = format!("agents/{agent_name}.{extension}");
        if !old_lock.contains_output(target, &dest_rel) {
            continue;
        }
        let native_path = project_root
            .join(target)
            .join("agents")
            .join(format!("{agent_name}.{extension}"));
        if !native_path.exists() && native_path.symlink_metadata().is_err() {
            continue;
        }
        if dry_run {
            continue;
        }
        if let Err(e) = crate::reconcile::fs_ops::safe_remove(&native_path) {
            diag.warn(
                "native-agent-remove",
                format!("could not remove {}: {e}", native_path.display()),
            );
        }
    }
}

struct NativeAgentSurfaceCompileOptions {
    force: bool,
    collision_hint: crate::surface_ownership::CollisionAdoptHint,
    dry_run: bool,
}

/// Shared inputs for dual-surface and selective native agent compilation.
struct NativeAgentCompileCtx<'a> {
    project_root: &'a Path,
    mars_dir: &'a Path,
    model_aliases: &'a IndexMap<String, crate::models::ModelAlias>,
    cursor_probe_slugs: &'a [String],
    old_lock: &'a crate::lock::LockFile,
    options: NativeAgentSurfaceCompileOptions,
}

/// One lowered native agent write request.
struct NativeAgentEmit<'a> {
    harness: &'a crate::compiler::agents::HarnessKind,
    profile: &'a crate::compiler::agents::AgentProfile,
    fm: &'a crate::frontmatter::Frontmatter,
    body: &'a str,
    agent_name: &'a str,
    model_override: Option<&'a str>,
}

struct NativeAgentEmitCtx<'a> {
    project_root: &'a Path,
    old_lock: &'a crate::lock::LockFile,
    options: &'a NativeAgentSurfaceCompileOptions,
}

/// Selective native emission: qualify agents per `agent_copy` harness allowlist.
fn selective_surface_compile(
    ctx: &NativeAgentCompileCtx<'_>,
    spec: &agent_copy::AgentCopySpec,
    diag: &mut DiagnosticCollector,
) -> Vec<(String, String, crate::types::ContentHash)> {
    use crate::compiler::agents::parse_agent_content;

    let emit_ctx = NativeAgentEmitCtx {
        project_root: ctx.project_root,
        old_lock: ctx.old_lock,
        options: &ctx.options,
    };
    let agents_dir = ctx.mars_dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                diag.warn(
                    "dual-surface-read",
                    format!("could not read {}: {e}", path.display()),
                );
                continue;
            }
        };

        let mut agent_diags = Vec::new();
        let (profile, fm) = match parse_agent_content(&content, &mut agent_diags) {
            Ok(r) => r,
            Err(e) => {
                diag.warn(
                    "dual-surface-parse",
                    format!("could not parse {}: {e}", path.display()),
                );
                continue;
            }
        };

        let agent_name = profile.name.as_deref().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
        });
        for d in &agent_diags {
            if d.is_error() {
                diag.warn(
                    "agent-schema-error",
                    format!("agent `{agent_name}`: {}", d.message()),
                );
            } else {
                diag.warn(
                    "agent-schema-warning",
                    format!("agent `{agent_name}`: {}", d.message()),
                );
            }
        }

        for harness in &spec.harnesses {
            let Some(emission) = agent_copy::agent_qualifies_for_harness(
                &profile,
                harness,
                ctx.model_aliases,
                spec.include_fanout,
            ) else {
                continue;
            };
            let model_override = agent_copy::model_override_for_emission(
                harness,
                &profile,
                &emission,
                ctx.model_aliases,
                ctx.cursor_probe_slugs,
            );
            emit_lowered_native_agent(
                &NativeAgentEmit {
                    harness,
                    profile: &profile,
                    fm: &fm,
                    body: fm.body(),
                    agent_name,
                    model_override: model_override.as_deref(),
                },
                &emit_ctx,
                diag,
                &mut records,
            );
        }
    }

    records
}

/// Dual-surface compilation: scan `.mars/agents/` for harness-bound agents and
/// emit native artifacts into the project root.
///
/// For each `*.md` file in `.mars/agents/`:
/// 1. Parse the agent profile frontmatter.
/// 2. Emit lossiness warnings for dropped fields.
/// 3. If `harness:` is set, lower to native format and write to
///    `<project_root>/<harness_dir>/agents/<name>.<ext>`.
///
/// Errors are non-fatal — they are emitted as diagnostics and the sync continues.
/// This preserves the "target sync is non-fatal" principle (D9).
fn dual_surface_compile(
    ctx: &NativeAgentCompileCtx<'_>,
    diag: &mut DiagnosticCollector,
) -> Vec<(String, String, crate::types::ContentHash)> {
    use crate::compiler::agents::parse_agent_content;

    let emit_ctx = NativeAgentEmitCtx {
        project_root: ctx.project_root,
        old_lock: ctx.old_lock,
        options: &ctx.options,
    };
    let agents_dir = ctx.mars_dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "md" {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                diag.warn(
                    "dual-surface-read",
                    format!("could not read {}: {e}", path.display()),
                );
                continue;
            }
        };

        let mut agent_diags = Vec::new();
        let (profile, fm) = match parse_agent_content(&content, &mut agent_diags) {
            Ok(r) => r,
            Err(e) => {
                diag.warn(
                    "dual-surface-parse",
                    format!("could not parse {}: {e}", path.display()),
                );
                continue;
            }
        };

        // Emit agent-level diagnostics (validation errors, legacy fields)
        let agent_name = profile.name.as_deref().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
        });
        for d in &agent_diags {
            if d.is_error() {
                diag.warn(
                    "agent-schema-error",
                    format!("agent `{agent_name}`: {}", d.message()),
                );
            } else {
                diag.warn(
                    "agent-schema-warning",
                    format!("agent `{agent_name}`: {}", d.message()),
                );
            }
        }

        // If no harness:, this is a universal agent — only .mars/ canonical output, done.
        let Some(harness) = &profile.harness else {
            continue;
        };

        let model_override = native_model_override_for_harness(
            harness,
            &profile,
            ctx.model_aliases,
            ctx.cursor_probe_slugs,
        );
        emit_lowered_native_agent(
            &NativeAgentEmit {
                harness,
                profile: &profile,
                fm: &fm,
                body: fm.body(),
                agent_name,
                model_override: model_override.as_deref(),
            },
            &emit_ctx,
            diag,
            &mut records,
        );
    }
    records
}

fn emit_lowered_native_agent(
    agent: &NativeAgentEmit<'_>,
    ctx: &NativeAgentEmitCtx<'_>,
    diag: &mut DiagnosticCollector,
    records: &mut Vec<(String, String, crate::types::ContentHash)>,
) {
    use crate::compiler::agents::lower::{lower_for_harness, lower_for_harness_with_model};
    use crate::surface_ownership::{self, SurfaceCopyDecision};

    let lowered = match agent.model_override {
        Some(model) => lower_for_harness_with_model(
            agent.harness,
            agent.profile,
            agent.fm,
            agent.body,
            Some(model),
        ),
        None => lower_for_harness(agent.harness, agent.profile, agent.fm, agent.body),
    };

    for lf in &lowered.lossy_fields {
        use crate::compiler::agents::lower::Lossiness;
        match &lf.classification {
            Lossiness::Dropped | Lossiness::MeridianOnly => {}
            Lossiness::Approximate { note } => {
                diag.warn(
                    "agent-field-approximate",
                    format!(
                        "agent `{}`: field `{}` approximately mapped in {} ({note})",
                        agent.agent_name, lf.field, lf.target
                    ),
                );
            }
        }
    }

    let harness_dir = ctx.project_root.join(agent.harness.target_dir());
    let native_agents_dir = harness_dir.join("agents");
    let file_name = match agent.harness {
        crate::compiler::agents::HarnessKind::Codex => format!("{}.toml", agent.agent_name),
        _ => format!("{}.md", agent.agent_name),
    };
    let native_path = native_agents_dir.join(&file_name);
    let dest_rel = format!("agents/{file_name}");
    let target_dir = agent.harness.target_dir();
    let dest_exists = surface_ownership::target_dest_exists(&native_path);
    match surface_ownership::copy_decision(
        ctx.old_lock,
        target_dir,
        &dest_rel,
        dest_exists,
        ctx.options.force,
    ) {
        SurfaceCopyDecision::SkipUnmanagedCollision => {
            surface_ownership::warn_unmanaged_collision(
                target_dir,
                &dest_rel,
                ctx.options.collision_hint,
                diag,
            );
            return;
        }
        SurfaceCopyDecision::Proceed => {
            if dest_exists
                && ctx.options.force
                && !ctx.old_lock.contains_output(target_dir, &dest_rel)
            {
                surface_ownership::warn_unmanaged_adopted(
                    target_dir,
                    &dest_rel,
                    ctx.options.collision_hint,
                    diag,
                );
            }
        }
    }

    if ctx.options.dry_run {
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&native_agents_dir) {
        diag.warn(
            "dual-surface-mkdir",
            format!("could not create {}: {e}", native_agents_dir.display()),
        );
        return;
    }

    if let Err(e) = crate::fs::atomic_write(&native_path, &lowered.bytes) {
        diag.warn(
            "dual-surface-write",
            format!("could not write {}: {e}", native_path.display()),
        );
    } else {
        let checksum = crate::types::ContentHash::from(crate::hash::hash_bytes(&lowered.bytes));
        records.push((target_dir.to_string(), dest_rel, checksum));
    }
}

fn merged_model_aliases_for_native_agents(
    resolved: &crate::sync::ResolvedState,
) -> IndexMap<String, crate::models::ModelAlias> {
    let mut local_diag = DiagnosticCollector::new();
    crate::models::merged_model_aliases(
        &resolved.graph,
        &resolved.loaded.effective,
        &resolved.loaded.config,
        &resolved.loaded.local,
        &mut local_diag,
    )
}

fn cached_cursor_probe_slugs_for_native_agents() -> Vec<String> {
    crate::models::probes::cursor_cache::read_cached_probe_result_usable()
        .map(|probe| probe.slugs)
        .unwrap_or_default()
}

pub(crate) fn native_model_override_for_harness(
    harness: &crate::compiler::agents::HarnessKind,
    profile: &crate::compiler::agents::AgentProfile,
    aliases: &IndexMap<String, crate::models::ModelAlias>,
    cursor_probe_slugs: &[String],
) -> Option<String> {
    if !matches!(harness, crate::compiler::agents::HarnessKind::Cursor) {
        return None;
    }
    map_cursor_native_model(profile, aliases, cursor_probe_slugs)
}

fn map_cursor_native_model(
    profile: &crate::compiler::agents::AgentProfile,
    aliases: &IndexMap<String, crate::models::ModelAlias>,
    cursor_probe_slugs: &[String],
) -> Option<String> {
    let token = profile.model.as_deref()?;
    if token.contains('[') {
        return None;
    }

    let alias = aliases.get(token);
    let model_id = alias.and_then(pinned_model_id).unwrap_or(token);
    let effort = cursor_effective_effort(profile, alias).unwrap_or("medium");
    if cursor_probe_slugs.is_empty() {
        return None;
    }

    for candidate in cursor_probe_lookup_model_ids(model_id) {
        if let Ok(resolution) = crate::models::probes::cursor::resolve_cursor_effort_slug(
            &candidate,
            effort,
            cursor_probe_slugs,
        ) {
            return Some(resolution.slug);
        }
    }

    None
}

fn pinned_model_id(alias: &crate::models::ModelAlias) -> Option<&str> {
    match &alias.spec {
        crate::models::ModelSpec::Pinned { model, .. }
        | crate::models::ModelSpec::PinnedWithMatch { model, .. } => Some(model.as_str()),
        crate::models::ModelSpec::AutoResolve { .. } => None,
    }
}

fn cursor_effective_effort<'a>(
    profile: &'a crate::compiler::agents::AgentProfile,
    alias: Option<&'a crate::models::ModelAlias>,
) -> Option<&'a str> {
    profile
        .harness_overrides
        .cursor
        .as_ref()
        .and_then(|overrides| overrides.effort.as_ref())
        .map(crate::compiler::agents::EffortLevel::as_str)
        .or_else(|| {
            profile
                .effort
                .as_ref()
                .map(crate::compiler::agents::EffortLevel::as_str)
        })
        .or_else(|| alias.and_then(|resolved| resolved.default_effort.as_deref()))
        .map(|effort| match effort {
            "auto" => "medium",
            other => other,
        })
}

fn cursor_probe_lookup_model_ids(model_id: &str) -> Vec<String> {
    let mut candidates = vec![model_id.to_string()];
    if let Some(shimmed) = cursor_probe_model_id_shim(model_id) {
        candidates.push(shimmed);
    }
    candidates
}

fn cursor_probe_model_id_shim(model_id: &str) -> Option<String> {
    match model_id.to_ascii_lowercase().as_str() {
        "claude-opus-4-6" => Some("claude-4.6-opus".to_string()),
        "claude-sonnet-4-6" => Some("claude-4.6-sonnet".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod skill_surface_tests {
    use super::*;
    use crate::compiler::agents::HarnessKind;
    use crate::diagnostic::DiagnosticCollector;
    use crate::lock::{ItemId, ItemKind, LockFile, LockedItemV2, OutputRecord};
    use crate::models::{ModelAlias, ModelSpec};
    use crate::sync::apply::{ActionOutcome, ActionTaken};
    use crate::types::{DestPath, ItemName};
    use indexmap::IndexMap;
    use tempfile::TempDir;

    #[test]
    fn native_agent_emission_defaults_to_standalone_auto() {
        assert_eq!(
            agent_surface_policy(None, None, false),
            AgentSurfacePolicy::EmitAll
        );
    }

    #[test]
    fn native_agent_emission_auto_suppresses_meridian_managed() {
        assert_eq!(
            agent_surface_policy(Some(&AgentEmission::Auto), None, true),
            AgentSurfacePolicy::SuppressAll
        );
    }

    #[test]
    fn native_agent_emission_always_ignores_meridian_managed() {
        assert_eq!(
            agent_surface_policy(Some(&AgentEmission::Always), None, true),
            AgentSurfacePolicy::EmitAll
        );
    }

    #[test]
    fn native_agent_emission_never_suppresses_standalone() {
        assert_eq!(
            agent_surface_policy(Some(&AgentEmission::Never), None, false),
            AgentSurfacePolicy::SuppressAll
        );
    }

    #[test]
    fn agent_copy_supersedes_meridian_managed_auto() {
        let spec = agent_copy::AgentCopySpec {
            harnesses: vec![HarnessKind::Claude],
            include_fanout: false,
        };
        assert!(matches!(
            agent_surface_policy(Some(&AgentEmission::Auto), Some(&spec), true),
            AgentSurfacePolicy::EmitSelective(_)
        ));
    }

    #[test]
    fn agent_copy_supersedes_never_emission() {
        let spec = agent_copy::AgentCopySpec {
            harnesses: vec![HarnessKind::Claude],
            include_fanout: false,
        };
        assert!(matches!(
            agent_surface_policy(Some(&AgentEmission::Never), Some(&spec), false),
            AgentSurfacePolicy::EmitSelective(_)
        ));
    }

    #[test]
    fn agent_emission_always_takes_precedence_over_agent_copy() {
        let spec = agent_copy::AgentCopySpec {
            harnesses: vec![HarnessKind::Claude],
            include_fanout: false,
        };
        assert_eq!(
            agent_surface_policy(Some(&AgentEmission::Always), Some(&spec), true),
            AgentSurfacePolicy::EmitAll
        );
    }

    fn profile_with_cursor_model(model: &str) -> crate::compiler::agents::AgentProfile {
        crate::compiler::agents::AgentProfile {
            name: None,
            description: None,
            harness: Some(HarnessKind::Cursor),
            model: Some(model.to_string()),
            mode: None,
            model_invocable: true,
            approval: None,
            sandbox: None,
            effort: None,
            autocompact: None,
            autocompact_pct: None,
            skills: crate::frontmatter::SkillsSpec::default(),
            subagents: Vec::new(),
            tools: Vec::new(),
            tools_denied: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_tools: Vec::new(),
            harness_overrides: crate::compiler::agents::HarnessOverrides::default(),
            model_policies: Vec::new(),
            fanout: Vec::new(),
        }
    }

    fn pinned_alias(model: &str, default_effort: Option<&str>) -> ModelAlias {
        ModelAlias {
            harness: Some("codex".to_string()),
            description: None,
            default_effort: default_effort.map(str::to_owned),
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: model.to_string(),
                provider: None,
            },
        }
    }

    #[test]
    fn cursor_native_model_mapping_uses_shared_resolver_for_alias_and_effort() {
        let profile = profile_with_cursor_model("gpt55");
        let mut aliases = IndexMap::new();
        aliases.insert("gpt55".to_string(), pinned_alias("gpt-5.5", Some("high")));
        let slugs = vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()];
        assert_eq!(
            native_model_override_for_harness(&HarnessKind::Cursor, &profile, &aliases, &slugs),
            Some("gpt-5.5-high".to_string())
        );
    }

    #[test]
    fn cursor_native_model_mapping_preserves_unknown_or_cursor_literal_tokens() {
        let profile = profile_with_cursor_model("composer-2.5[fast=false]");
        let slugs = vec!["composer-2.5".to_string(), "composer-2.5-fast".to_string()];
        assert_eq!(
            native_model_override_for_harness(
                &HarnessKind::Cursor,
                &profile,
                &IndexMap::new(),
                &slugs
            ),
            None
        );

        let profile = profile_with_cursor_model("unmapped-model");
        assert_eq!(
            native_model_override_for_harness(
                &HarnessKind::Cursor,
                &profile,
                &IndexMap::new(),
                &slugs
            ),
            None
        );
    }

    #[test]
    fn cursor_native_model_mapping_uses_claude_shim_with_shared_resolver() {
        let profile = profile_with_cursor_model("opus");
        let mut aliases = IndexMap::new();
        aliases.insert(
            "opus".to_string(),
            pinned_alias("claude-opus-4-6", Some("high")),
        );
        let slugs = vec![
            "claude-4.6-opus-thinking-high".to_string(),
            "claude-4.6-opus-thinking-medium".to_string(),
        ];

        assert_eq!(
            native_model_override_for_harness(&HarnessKind::Cursor, &profile, &aliases, &slugs),
            Some("claude-4.6-opus-thinking-high".to_string())
        );
    }

    fn lock_with_target_outputs(targets: &[&str], dest: &str, checksum: &str) -> LockFile {
        let mut lock = LockFile::empty();
        let outputs = targets
            .iter()
            .map(|target| OutputRecord {
                target_root: (*target).to_string(),
                dest_path: dest.into(),
                installed_checksum: checksum.into(),
            })
            .collect();
        lock.items.insert(
            "agent/coder".to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs,
            },
        );
        lock
    }

    fn agent_outcome(name: &str, action: ActionTaken) -> ActionOutcome {
        ActionOutcome {
            item_id: ItemId {
                kind: ItemKind::Agent,
                name: ItemName::from(name),
            },
            action,
            dest_path: DestPath::from(format!("agents/{name}.md")),
            source_name: "test-source".into(),
            source_checksum: None,
            installed_checksum: None,
        }
    }

    #[test]
    fn reconcile_emit_all_removes_native_shapes_for_removed_agents() {
        let dir = TempDir::new().unwrap();
        for harness in HarnessKind::all() {
            let agents_dir = dir.path().join(harness.target_dir()).join("agents");
            std::fs::create_dir_all(&agents_dir).unwrap();
            std::fs::write(agents_dir.join("coder.md"), "# Old\n").unwrap();
            std::fs::write(agents_dir.join("coder.toml"), "old = true\n").unwrap();
        }

        let tracked_targets: Vec<&str> =
            HarnessKind::all().iter().map(|h| h.target_dir()).collect();
        let mut lock =
            lock_with_target_outputs(&tracked_targets, "agents/coder.md", "sha256:coder");
        for target in &tracked_targets {
            lock.items
                .get_mut("agent/coder")
                .unwrap()
                .outputs
                .push(OutputRecord {
                    target_root: (*target).to_string(),
                    dest_path: "agents/coder.toml".into(),
                    installed_checksum: "sha256:coder-toml".into(),
                });
        }

        let mut diag = DiagnosticCollector::new();
        reconcile_native_agent_surfaces(
            &NativeAgentReconcileCtx {
                policy: AgentSurfacePolicy::EmitAll,
                project_root: dir.path(),
                mars_dir: &dir.path().join(".mars"),
                model_aliases: &IndexMap::new(),
                outcomes: &[agent_outcome("coder", ActionTaken::Removed)],
                old_lock: &lock,
                dry_run: false,
            },
            &mut diag,
        );

        for harness in HarnessKind::all() {
            assert!(
                !dir.path()
                    .join(harness.target_dir())
                    .join("agents/coder.md")
                    .exists()
            );
            assert!(
                !dir.path()
                    .join(harness.target_dir())
                    .join("agents/coder.toml")
                    .exists()
            );
        }
        assert!(diag.drain().is_empty());
    }

    #[test]
    fn reconcile_suppress_all_removes_native_shapes_for_current_agents() {
        let dir = TempDir::new().unwrap();

        // Set up a canonical agent in .mars/agents/
        let mars_agents = dir.path().join(".mars").join("agents");
        std::fs::create_dir_all(&mars_agents).unwrap();
        std::fs::write(
            mars_agents.join("coder.md"),
            "---\nname: coder\n---\n# Coder\n",
        )
        .unwrap();

        // Set up native artifacts that should be cleaned
        for target in [".claude", ".codex", ".opencode"] {
            let agents_dir = dir.path().join(target).join("agents");
            std::fs::create_dir_all(&agents_dir).unwrap();
            std::fs::write(agents_dir.join("coder.md"), "# Native\n").unwrap();
        }

        let mut diag = DiagnosticCollector::new();
        let lock = lock_with_target_outputs(
            &[".claude", ".codex", ".opencode"],
            "agents/coder.md",
            "sha256:coder",
        );
        reconcile_native_agent_surfaces(
            &NativeAgentReconcileCtx {
                policy: AgentSurfacePolicy::SuppressAll,
                project_root: dir.path(),
                mars_dir: &dir.path().join(".mars"),
                model_aliases: &IndexMap::new(),
                outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
                old_lock: &lock,
                dry_run: false,
            },
            &mut diag,
        );

        for target in [".claude", ".codex", ".opencode"] {
            assert!(
                !dir.path().join(target).join("agents/coder.md").exists(),
                "native agent should be removed under SuppressAll for target {target}"
            );
        }
    }

    #[test]
    fn reconcile_suppress_all_preserves_untracked_native_agents() {
        let dir = TempDir::new().unwrap();

        let mars_agents = dir.path().join(".mars").join("agents");
        std::fs::create_dir_all(&mars_agents).unwrap();
        std::fs::write(
            mars_agents.join("coder.md"),
            "---\nname: coder\n---\n# Coder\n",
        )
        .unwrap();

        let agents_dir = dir.path().join(".cursor").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("coder.md"), "# hand-written\n").unwrap();

        let mut diag = DiagnosticCollector::new();
        reconcile_native_agent_surfaces(
            &NativeAgentReconcileCtx {
                policy: AgentSurfacePolicy::SuppressAll,
                project_root: dir.path(),
                mars_dir: &dir.path().join(".mars"),
                model_aliases: &IndexMap::new(),
                outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
                old_lock: &LockFile::empty(),
                dry_run: false,
            },
            &mut diag,
        );

        assert!(dir.path().join(".cursor/agents/coder.md").exists());
    }

    #[test]
    fn reconcile_selective_removes_native_when_agent_stops_qualifying() {
        let dir = TempDir::new().unwrap();
        let mars_agents = dir.path().join(".mars").join("agents");
        std::fs::create_dir_all(&mars_agents).unwrap();
        std::fs::write(
            mars_agents.join("coder.md"),
            "---\nname: coder\nmodel: opus\n---\n# Coder\n",
        )
        .unwrap();

        let claude_agents = dir.path().join(".claude").join("agents");
        std::fs::create_dir_all(&claude_agents).unwrap();
        std::fs::write(claude_agents.join("coder.md"), "# Native\n").unwrap();

        let spec = agent_copy::AgentCopySpec {
            harnesses: vec![HarnessKind::Claude],
            include_fanout: false,
        };
        let mut aliases = IndexMap::new();
        aliases.insert(
            "opus".to_string(),
            ModelAlias {
                harness: None,
                description: None,
                default_effort: None,
                autocompact: None,
                autocompact_pct: None,
                spec: ModelSpec::Pinned {
                    model: "claude-opus-4-6".to_string(),
                    provider: Some("openai".to_string()),
                },
            },
        );

        let lock = lock_with_target_outputs(&[".claude"], "agents/coder.md", "sha256:coder");
        let mut diag = DiagnosticCollector::new();
        reconcile_native_agent_surfaces(
            &NativeAgentReconcileCtx {
                policy: AgentSurfacePolicy::EmitSelective(spec),
                project_root: dir.path(),
                mars_dir: &dir.path().join(".mars"),
                model_aliases: &aliases,
                outcomes: &[],
                old_lock: &lock,
                dry_run: false,
            },
            &mut diag,
        );

        assert!(
            !dir.path().join(".claude/agents/coder.md").exists(),
            "openai-bound model should not qualify for claude selective reconcile"
        );
    }

    #[test]
    fn reconcile_emit_all_preserves_non_removed_agents() {
        let dir = TempDir::new().unwrap();

        // Set up native artifacts for a non-removed agent
        let agents_dir = dir.path().join(".claude").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("coder.md"), "# Native\n").unwrap();

        let mut diag = DiagnosticCollector::new();
        reconcile_native_agent_surfaces(
            &NativeAgentReconcileCtx {
                policy: AgentSurfacePolicy::EmitAll,
                project_root: dir.path(),
                mars_dir: &dir.path().join(".mars"),
                model_aliases: &IndexMap::new(),
                outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
                old_lock: &LockFile::empty(),
                dry_run: false,
            },
            &mut diag,
        );

        // Native artifact should still exist
        assert!(dir.path().join(".claude/agents/coder.md").exists());
    }
}
