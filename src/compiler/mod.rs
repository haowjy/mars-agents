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
pub mod context;
/// Hook compiler lane: discovery, event validation, ordering, lossiness classification.
pub mod hooks;
/// MCP server compiler lane: discovery, env-ref validation, collision detection.
pub mod mcp;
pub(crate) mod native_agent_manifest;
mod native_agents;
/// Skill frontmatter compiler lane: universal schema parsing and native lowering.
pub mod skills;
/// Skill variant layout validation, indexing, and projection helpers.
pub mod variants;
/// Visibility propagation rules for passive vs effectful items (D1/D10).
pub mod visibility;

pub use native_agent_manifest::persist_lock_then_native_agent_manifest;
pub use native_agents::selective_native_orphan_preserve_paths;
pub(crate) use native_agents::{
    NativeAgentLinkMaterializeCtx, RemovedNativeOutput, materialize_native_agents_after_link,
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
    let configured_emit_harnesses: Vec<agents::HarnessKind> = effective_settings
        .managed_targets()
        .iter()
        .filter_map(|t| agents::HarnessKind::from_target_dir(t))
        .collect();
    let mars_dir = ctx.project_root.join(".mars");
    let models_cache =
        crate::models::read_cache(&mars_dir).unwrap_or_else(|_| crate::models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        });
    let model_aliases =
        native_agents::merged_model_aliases_for_native_agents(&applied.planned.targeted.resolved);
    let cursor_probe_slugs = native_agents::cached_cursor_probe_slugs_for_native_agents();
    let mars_agents = native_agents::scan_mars_agents(&mars_dir, diag);

    // Phase 5.1 / 5.2 / 5.3: MCP and hooks config-entry compilation.
    let config_entry_records =
        config_entries::compile_config_entries(ctx, &applied, request.options.dry_run, diag);

    // Phase 6: copy from canonical store to managed target directories.
    let mut synced = sync_targets(ctx, applied, request, agent_surface_policy.clone(), diag);
    synced.config_entries = config_entry_records;

    let old_lock = &synced.applied.planned.targeted.resolved.loaded.old_lock;
    let outcomes = &synced.applied.applied.outcomes;
    let loaded = &synced.applied.planned.targeted.resolved.loaded;
    // Per-agent overlays merged from mars.toml + mars.local.toml (loaded.effective is
    // EffectiveConfig, which does not carry the overlay map).
    let agent_overlays = crate::config::merged_agent_overlays(&loaded.config.agents, &loaded.local);
    let ownership_lock;
    let native_ownership_lock = if request.options.dry_run {
        old_lock
    } else {
        ownership_lock = crate::lock::ownership_lock_for_native_emission(
            old_lock,
            outcomes,
            &synced.target_outcomes,
        );
        &ownership_lock
    };
    let native_reconcile_ctx = native_agents::NativeAgentReconcileCtx {
        policy: agent_surface_policy.clone(),
        project_root: &ctx.project_root,
        model_aliases: &model_aliases,
        outcomes,
        old_lock,
        dry_run: request.options.dry_run,
        selective_harness_scope: None,
    };
    let native_compile_ctx = if matches!(agent_surface_policy, AgentSurfacePolicy::SuppressAll) {
        None
    } else {
        Some(native_agents::NativeAgentCompileCtx {
            project_root: &ctx.project_root,
            model_aliases: &model_aliases,
            models_cache: &models_cache,
            cursor_probe_slugs: &cursor_probe_slugs,
            old_lock: native_ownership_lock,
            harness_scope: None,
            configured_emit_harnesses: &configured_emit_harnesses,
            options: native_agents::NativeAgentSurfaceCompileOptions {
                force: request.options.force,
                collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
                dry_run: request.options.dry_run,
            },
        })
    };
    (
        synced.compiled_native_outputs,
        synced.removed_native_outputs,
    ) = native_agents::run_native_agent_post_sync_lifecycle(
        &native_reconcile_ctx,
        &agent_surface_policy,
        &mars_agents,
        &agent_overlays,
        native_compile_ctx.as_ref(),
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
            if outcome.item_id.kind == crate::lock::ItemKind::Agent {
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
        .filter(|outcome| outcome.item_id.kind != crate::lock::ItemKind::Agent)
        .cloned()
        .collect()
}

#[cfg(test)]
mod skill_surface_tests {
    use super::*;
    use crate::compiler::agents::HarnessKind;
    use crate::config::AgentEmission;
    use crate::lock::{ItemId, ItemKind};
    use crate::sync::apply::{ActionOutcome, ActionTaken};
    use crate::types::{DestPath, ItemName};

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
}
