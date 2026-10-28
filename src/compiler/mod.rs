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
/// Typed native harness descriptor table shared by compiler lowering lanes.
pub(crate) mod harness_descriptor;
pub mod hooks;
/// Hook compiler lane: native event discovery, validation, and ordering.
pub(crate) mod lossiness;
pub(crate) mod lossiness_preview;
/// MCP server compiler lane: discovery, env-ref validation, collision detection.
pub mod mcp;
pub(crate) mod mcp_ref;
pub(crate) mod native_agent_manifest;
mod native_agents;
/// Skill frontmatter compiler lane: universal schema parsing and native lowering.
pub mod skills;
pub(crate) mod tool_policy;
/// Skill variant layout validation, indexing, and projection helpers.
pub mod variants;

/// Item names become one directory under a target; reject anything that could
/// escape it.
pub(crate) fn validate_path_component(name: &str) -> Result<(), &'static str> {
    use std::path::Component;
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("must be a single path component");
    }
    let mut components = std::path::Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err("must be a single path component"),
    }
}

/// Say that an item's `visibility` is not `local` or `exported`; it is read as `local`.
pub(crate) fn warn_unknown_visibility(
    unknown: &str,
    item_kind: &str,
    item_name: &str,
    source_name: &str,
    diag: &mut DiagnosticCollector,
) {
    diag.warn(
        "unknown-visibility",
        format!(
            "{item_kind} `{item_name}` from source `{source_name}` has unknown \
             visibility `{unknown}`; treating it as `local`"
        ),
    );
}

pub use native_agent_manifest::persist_lock_then_native_agent_manifest;
pub(crate) use native_agents::{
    NativeAgentLinkMaterializeCtx, RemovedNativeOutput, effective_native_profile,
    materialize_native_agents_after_link,
};
pub use native_agents::{
    native_agent_orphan_preserve_paths, selective_native_orphan_preserve_paths,
};

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
    // Phase 3: assign dest paths, handle collisions, rewrite frontmatter refs.
    let targeted = build_target(ctx, ir.resolved, ir.local_items, request, diag)?;

    if enforce_diagnostic_gate {
        diag.fail_on_errors()?;
    }
    // Phase 4: diff + plan.
    let planned = create_plan(ctx, targeted, request, diag)?;

    // Frozen gate: no pending changes allowed.
    if request.options.frozen {
        check_frozen_gate(&planned)?;
    }

    // Hook schemas and native allowlists are resolved before any mutation.
    config_entries::preflight_config_entries(ctx, &planned.targeted, diag)?;

    if enforce_diagnostic_gate {
        diag.fail_on_errors()?;
    }
    // Phase 5: persist config mutations, apply plan to canonical store.
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

    // Phase 3.2 / 3.3: Native agent surfaces — scan once; reconcile + compile after target sync.
    let effective_settings = &applied.planned.targeted.resolved.loaded.effective.settings;
    let agent_copy_spec = agent_copy::build_agent_copy_spec(
        effective_settings.meridian_agent_copy(),
        &effective_settings.managed_targets(),
        diag,
    );
    let agent_surface_policy = agent_surface_policy(
        effective_settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        ctx.meridian_managed,
    );
    let configured_emit_harnesses =
        harness_descriptor::configured_emit_harnesses(effective_settings);
    let mars_dir = ctx.project_root.join(".mars");
    let models_cache =
        crate::models::read_cache(&mars_dir).unwrap_or_else(|_| crate::models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        });
    let model_aliases =
        native_agents::merged_model_aliases_for_native_agents(&applied.planned.targeted.resolved);
    let routing_settings =
        crate::config::routing_settings::ResolvedRoutingSettings::from_settings(effective_settings);
    let mut native_model_router =
        (!matches!(agent_surface_policy, AgentSurfacePolicy::SuppressAll)).then(|| {
            native_agents::NativeModelRoutingRuntime::collect(
                &model_aliases,
                &models_cache,
                routing_settings,
            )
            .with_settings(effective_settings.clone())
        });
    let mars_agent_scan = native_agents::scan_mars_agents(&mars_dir, diag);

    // Phase 6: install canonical content before emitting config that refers to it.
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
        request.options.dry_run,
        request.options.force,
        diag,
    );
    synced.config_entries = config_entry_compilation.records;
    synced.config_entry_outputs = config_entry_compilation.emitted_outputs;
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
    let native_compile_ctx = if matches!(agent_surface_policy, AgentSurfacePolicy::SuppressAll) {
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
        &loaded.effective.agents,
        native_compile_ctx.as_ref(),
        native_model_router.as_mut(),
        diag,
    );

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

#[cfg(test)]
mod skill_surface_tests {
    use super::*;
    use crate::config::AgentEmission;
    use crate::lock::ItemId;
    use crate::sync::apply::{ActionOutcome, ActionTaken};
    use crate::types::DestPath;
    use mars_schema::HarnessId;
    use mars_schema::item::ItemKind;
    use mars_schema::names::ItemName;

    /// The `agent_emission` / `agent_copy` / Meridian-managed table in `compiler/AGENTS.md`.
    #[test]
    fn agent_surface_policy_follows_the_documented_table() {
        let copy = agent_copy::AgentCopySpec {
            harnesses: vec![HarnessId::Claude],
            include_fanout: false,
        };
        let selective = AgentSurfacePolicy::EmitSelective(copy.clone());
        let auto = Some(&AgentEmission::Auto);
        let always = Some(&AgentEmission::Always);
        let never = Some(&AgentEmission::Never);
        let cases = [
            // (emission, agent_copy, meridian_managed, expected)
            (None, None, false, AgentSurfacePolicy::EmitAll),
            (auto, None, true, AgentSurfacePolicy::SuppressAll),
            (always, None, true, AgentSurfacePolicy::EmitAll),
            (never, None, false, AgentSurfacePolicy::SuppressAll),
            // agent_copy supersedes auto-under-Meridian and never, but not always.
            (auto, Some(&copy), true, selective.clone()),
            (never, Some(&copy), false, selective),
            (always, Some(&copy), true, AgentSurfacePolicy::EmitAll),
        ];
        for (emission, spec, managed, expected) in cases {
            assert_eq!(
                agent_surface_policy(emission, spec, managed),
                expected,
                "emission {emission:?}, agent_copy {spec:?}, managed {managed}"
            );
        }
    }

    #[test]
    fn omit_agent_outcomes_drops_agents_only() {
        let outcomes = vec![
            ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Agent,
                    name: ItemName::from("coder"),
                },
                dest_path: DestPath::from("agents/coder.md"),
                action: ActionTaken::Installed,
                source_name: "test-source".into(),
                source_checksum: None,
                installed_checksum: None,
            },
            ActionOutcome {
                item_id: ItemId {
                    kind: ItemKind::Skill,
                    name: ItemName::from("plan"),
                },
                dest_path: DestPath::from("skills/plan"),
                action: ActionTaken::Installed,
                source_name: "test-source".into(),
                source_checksum: None,
                installed_checksum: None,
            },
        ];
        let filtered = omit_agent_outcomes(&outcomes);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].item_id.kind, ItemKind::Skill);
    }
}
