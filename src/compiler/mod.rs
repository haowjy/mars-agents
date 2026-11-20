/// Selective native agent emission via `settings.meridian.agent_copy`.
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
pub(crate) mod effective_tools;
/// Typed native harness descriptor table shared by compiler lowering lanes.
pub mod hooks;
/// Hook compiler lane: native event discovery, validation, and ordering.
pub(crate) mod lossiness;
pub(crate) mod lossiness_preview;
pub(crate) mod lowering;
/// MCP server compiler lane: discovery, env-ref validation, collision detection.
pub mod mcp;
pub(crate) mod native_agent_manifest;
mod native_agents;
/// Skill frontmatter compiler lane: universal schema parsing and native lowering.
pub mod skills;
pub(crate) mod tool_notices;
/// Skill variant layout validation, indexing, and projection helpers.
pub mod variants;

/// Say that an item's `visibility` is not `local` or `exported`; it is read as `local`.
pub(crate) fn warn_unknown_visibility(
    unknown: &str,
    item_kind: &str,
    item_name: &str,
    source_name: &str,
    diag: &mut DiagnosticCollector,
) {
    diag.warn_advisory(
        "unknown-visibility",
        format!(
            "{item_kind} `{item_name}` from source `{source_name}` has unknown \
             visibility `{unknown}`; treating it as `local`"
        ),
    );
}

pub use native_agent_manifest::persist_lock_then_native_agent_manifest;
pub(crate) use native_agents::{
    NativeAgentLinkMaterializeCtx, RemovedNativeOutput, ToolConfig, effective_native_profile,
    installed_agent_tools, materialize_native_agents_after_link,
};
pub use native_agents::native_agent_orphan_preserve_paths;

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
    compile_inner(ctx, ir, request, diag, true)
}

pub(crate) fn compile_reporting(
    ctx: &MarsContext,
    ir: ReaderIr,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<SyncReport, MarsError> {
    compile_inner(ctx, ir, request, diag, false)
}

fn compile_inner(
    ctx: &MarsContext,
    ir: ReaderIr,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
    enforce_diagnostic_gate: bool,
) -> Result<SyncReport, MarsError> {
    let refresh = crate::models::resolve_models_refresh_control(
        request.options.refresh_models,
        request.options.no_refresh_models,
    )?;
    let targeted = build_target(ctx, ir.resolved, ir.local_items, request, diag)?;

    if enforce_diagnostic_gate {
        diag.fail_on_errors()?;
    }
    let planned = create_plan(ctx, targeted, request, diag)?;

    // Frozen gate: no pending changes allowed.
    if request.options.frozen {
        check_frozen_gate(&planned)?;
    }

    // Hook schemas and native allowlists are resolved before any mutation.
    let rendered_hooks =
        config_entries::render::preflight_config_entries(ctx, &planned.targeted, diag)?;

    if enforce_diagnostic_gate {
        diag.fail_on_errors()?;
    }
    let mut applied = apply_plan(ctx, planned, request)?;
    if !request.options.dry_run {
        let pending = applied
            .planned
            .targeted
            .resolved
            .preparation
            .path()
            .to_path_buf();
        let published = ctx.project_root.join(".mars/staging");
        crate::platform::fs::publish_staging_generation(&pending, &ctx.project_root.join(".mars"))?;
        applied
            .planned
            .targeted
            .target
            .rebase_source_root(&pending, &published);
        for node in applied.planned.targeted.resolved.graph.nodes.values_mut() {
            if let Ok(relative) = node.rooted_ref.package_root.strip_prefix(&pending) {
                node.rooted_ref.package_root = published.join(relative);
            }
        }
    }

    // Scan native agent surfaces once; reconcile and compile after target sync.
    let effective_settings = &applied.planned.targeted.resolved.loaded.effective.settings;
    let agent_copy_spec = agent_copy::build_agent_copy_spec(
        effective_settings.meridian_agent_copy(),
        &crate::config::targets::managed_targets(effective_settings),
        diag,
    );
    let agent_surface_policy = agent_surface_policy(
        effective_settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        ctx.meridian_managed,
    );
    let configured_emit_harnesses =
        crate::config::targets::configured_emit_harnesses(effective_settings);
    let mars_dir = ctx.project_root.join(".mars");
    let catalog_providers = effective_settings
        .catalog_providers
        .clone()
        .unwrap_or_else(crate::models::default_catalog_providers);
    // The catalog is settled here, before any model or effort is resolved against it: a refresh
    // after the agents are written would leave them lowered against the catalog it replaced.
    let models_cache = if request.options.dry_run {
        crate::models::read_cache(&mars_dir, &catalog_providers).unwrap_or_default()
    } else {
        refreshed_models_cache(
            &mars_dir,
            effective_settings.models_cache_ttl_hours(),
            refresh.catalog_mode,
            &catalog_providers,
            diag,
        )
    };
    let model_aliases =
        native_agents::merged_model_aliases_for_native_agents(&applied.planned.targeted.resolved);
    let routing_settings =
        crate::config::routing_settings::ResolvedRoutingSettings::from_settings(effective_settings);
    let mut native_model_router = (!agent_surface_policy.emits_nothing()).then(|| {
        native_agents::NativeModelRoutingRuntime::collect(
            &model_aliases,
            &models_cache,
            routing_settings,
        )
        .with_settings(effective_settings.clone())
    });
    let mars_agent_scan = native_agents::scan_mars_agents(&mars_dir, diag);
    // Nothing is lost: the stale-cache line already says the catalog is refreshing, so this
    // is not repeated beside it.
    if mars_agent_scan.has_agents()
        && models_cache.reasoning_data_missing()
        && !diag.has_code("models-cache-refresh")
    {
        diag.info(
            "models-reasoning-missing",
            "the models catalog has no reasoning data yet, so Agent efforts are written as given, \
             without checking the model; it fills in at the next successful refresh",
        );
    }

    // Install canonical content before emitting config that refers to it.
    let mut synced = sync_targets(ctx, applied, request, agent_surface_policy.clone(), diag);
    let old_lock = &synced.applied.planned.targeted.resolved.loaded.old_lock;
    let outcomes = &synced.applied.applied.outcomes;
    let mut ownership_lock = crate::lock::ownership_lock_for_native_emission(
        old_lock,
        outcomes,
        &synced.target_outcomes,
    );

    // Post-target-sync MCP and hook config-entry compilation. Merge-mode hook
    // bindings are emitted only after their directories are installed.
    let config_entry_compilation = config_entries::compile_config_entries(
        ctx,
        &synced.applied,
        &mut ownership_lock,
        &synced.target_outcomes,
        &rendered_hooks,
        request.options.dry_run,
        request.options.force,
        request.options.preview_warnings,
        diag,
    );
    synced.config_entries = config_entry_compilation.records;
    synced.config_entry_outputs = config_entry_compilation.emitted_outputs;
    synced.rewritten_hook_files = config_entry_compilation.rewritten_outputs;
    synced.config_files_changed = config_entry_compilation.config_files_changed;
    synced.removed_config_entry_outputs = config_entry_compilation.removed_outputs;

    let loaded = &synced.applied.planned.targeted.resolved.loaded;
    let native_ownership_lock = if request.options.dry_run {
        old_lock
    } else {
        &ownership_lock
    };
    let fanout_agents = loaded.effective.settings.meridian_fanout_agents();
    let native_reconcile_ctx = native_agents::NativeAgentReconcileCtx {
        project_root: &ctx.project_root,
        old_lock,
        dry_run: request.options.dry_run,
        selective_harness_scope: None,
    };
    let native_compile_ctx = if agent_surface_policy.emits_nothing() {
        None
    } else {
        Some(native_agents::NativeAgentCompileCtx {
            project_root: &ctx.project_root,
            old_lock: native_ownership_lock,
            harness_scope: None,
            configured_emit_harnesses: &configured_emit_harnesses,
            options: native_agents::NativeAgentSurfaceCompileOptions {
                force: request.options.force,
                collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
                dry_run: request.options.dry_run,
                preview_warnings: request.options.preview_warnings,
            },
            fanout_agents,
        })
    };
    (
        synced.compiled_native_outputs,
        synced.removed_native_outputs,
    ) = native_agents::run_native_agent_post_sync_lifecycle(
        &native_reconcile_ctx,
        &agent_surface_policy,
        &mars_agent_scan,
        native_agents::ToolConfig {
            agent_overlays: &loaded.effective.agents,
            project_tools: &loaded.effective.settings.tools,
        },
        native_compile_ctx.as_ref(),
        native_model_router.as_mut(),
        diag,
    );

    finalize(ctx, synced, request, diag)
}

/// What happens to agent artifacts on target surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSurfacePolicy {
    /// What applies to canonical agent copies and to the native agents of harnesses Meridian
    /// launches (`launched_by_meridian`).
    pub launched: AgentSurfaceMode,
    /// Whether the native agents of a harness Meridian does not launch are written: a harness
    /// run by hand has no other source for its agents, so only `agent_emission = "never"`
    /// withholds them.
    pub emit_unlaunched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSurfaceMode {
    /// Emit lowered native agents and copy canonical agents to managed targets.
    EmitAll,
    /// Emit only agents whose model resolves to configured harnesses.
    EmitSelective(agent_copy::AgentCopySpec),
    /// Suppress all agent artifacts on target surfaces.
    SuppressAll,
}

impl AgentSurfacePolicy {
    /// No harness gets a native agent.
    pub fn emits_nothing(&self) -> bool {
        self.launched == AgentSurfaceMode::SuppressAll && !self.emit_unlaunched
    }
}

pub fn agent_surface_policy(
    agent_emission: Option<&AgentEmission>,
    agent_copy: Option<&agent_copy::AgentCopySpec>,
    meridian_managed: bool,
) -> AgentSurfacePolicy {
    let under_meridian = || match agent_copy {
        Some(spec) if !spec.harnesses.is_empty() => AgentSurfaceMode::EmitSelective(spec.clone()),
        _ => AgentSurfaceMode::SuppressAll,
    };
    let (launched, emit_unlaunched) = match agent_emission.unwrap_or(&AgentEmission::Auto) {
        AgentEmission::Always => (AgentSurfaceMode::EmitAll, true),
        AgentEmission::Auto if !meridian_managed => (AgentSurfaceMode::EmitAll, true),
        AgentEmission::Auto => (under_meridian(), true),
        AgentEmission::Never => (under_meridian(), false),
    };
    AgentSurfacePolicy {
        launched,
        emit_unlaunched,
    }
}

/// Convert agent outcomes into removals so target sync removes canonical agent
/// surfaces under `SuppressAll`.
pub fn suppress_agent_outcomes(outcomes: &[ActionOutcome]) -> Vec<ActionOutcome> {
    outcomes
        .iter()
        .cloned()
        .map(|mut outcome| {
            if outcome.item_id.kind == mars_schema::item::ItemKind::Agent {
                outcome.action = ActionTaken::Removed;
            }
            outcome
        })
        .collect()
}

/// Drop agent outcomes from target sync under `EmitSelective`.
///
/// Native agent surfaces are owned by reconcile + selective compile, not target
/// sync. Skipped outcomes are unsafe here because target sync may copy canonical
/// agents when the destination is missing.
pub fn omit_agent_outcomes(outcomes: &[ActionOutcome]) -> Vec<ActionOutcome> {
    outcomes
        .iter()
        .filter(|outcome| outcome.item_id.kind != mars_schema::item::ItemKind::Agent)
        .cloned()
        .collect()
}

/// Target-sync `targets` from `outcomes` under `policy`: the one place that decides which
/// outcomes target sync sees and which lock-recorded files its orphan pass must leave alone
/// (hook-generated files, native agents that reconcile owns). Sync and `link` both call it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_targets_under_policy(
    project_root: &std::path::Path,
    targets: &[String],
    outcomes: &[ActionOutcome],
    policy: &AgentSurfacePolicy,
    old_lock: &crate::lock::LockFile,
    force: bool,
    collision_hint: crate::surface_ownership::CollisionAdoptHint,
    diag: &mut DiagnosticCollector,
) -> Vec<crate::target_sync::TargetSyncOutcome> {
    let filtered;
    let outcomes = match &policy.launched {
        AgentSurfaceMode::SuppressAll => {
            filtered = suppress_agent_outcomes(outcomes);
            &filtered
        }
        AgentSurfaceMode::EmitSelective(_) => {
            filtered = omit_agent_outcomes(outcomes);
            &filtered
        }
        AgentSurfaceMode::EmitAll => outcomes,
    };
    let mut preserve = config_entries::file_hooks::file_hook_output_preserve_paths(old_lock);
    for (target, paths) in native_agent_orphan_preserve_paths(old_lock, targets) {
        preserve.entry(target).or_default().extend(paths);
    }
    let ctx = crate::target_sync::TargetSyncContext {
        old_lock,
        force,
        collision_hint,
        orphan_preserve_paths: (!preserve.is_empty()).then_some(&preserve),
    };
    crate::target_sync::sync_managed_targets(
        project_root,
        &project_root.join(".mars"),
        targets,
        outcomes,
        &ctx,
        diag,
    )
}

/// The models catalog, refreshed per `mode`, with the refresh's warning on `diag`.
fn refreshed_models_cache(
    mars_dir: &std::path::Path,
    ttl_hours: u32,
    mode: crate::models::RefreshMode,
    providers: &[String],
    diag: &mut DiagnosticCollector,
) -> crate::models::ModelsCache {
    match crate::models::ensure_fresh_with_catalog_providers(mars_dir, ttl_hours, mode, providers) {
        Ok((cache, outcome)) => {
            diag.extend(
                crate::models::refresh_diagnostic(&outcome)
                    .into_iter()
                    .collect(),
            );
            cache
        }
        Err(err) => {
            diag.warn_advisory(
                "models-cache-refresh",
                format!(
                    "failed to refresh models cache: {err}; Efforts are written as given, without checking the model."
                ),
            );
            crate::models::ModelsCache::default()
        }
    }
}
