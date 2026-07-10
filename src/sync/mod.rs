pub mod apply;
pub mod diff;
pub mod filter;
pub mod mutation;
pub mod plan;
pub mod provider;
pub mod rewrite;
pub mod target;
pub mod types;
mod upgrades;
mod validate;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use crate::config::{Config, EffectiveConfig, LocalConfig, Settings};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, LossinessMode};
use crate::error::MarsError;
use crate::fs::FileLock;
use crate::hash;
use crate::lock::{CANONICAL_TARGET_ROOT, ItemId, ItemKind};
use crate::lock::{LockFile, LockIndex};
use crate::resolve::{ResolveOptions, ResolvedGraph};
use crate::source::GlobalCache;
use crate::sync::apply::ApplyResult;
pub use crate::sync::apply::SyncOptions;
use crate::sync::target::{TargetItem, TargetState};
use crate::types::managed_cmd;
use crate::types::{ContentHash, DestPath, MarsContext, SourceId, SourceName, SourceOrigin};
use crate::validate::ValidationWarning;

// Re-export mutation types for public API compatibility.
pub use crate::sync::mutation::{ConfigMutation, DependencyUpsertChange, apply_config_mutation};

/// Report from a completed sync operation.
#[derive(Debug)]
pub struct SyncReport {
    pub applied: ApplyResult,
    pub pruned: Vec<apply::ActionOutcome>,
    pub diagnostics: Vec<Diagnostic>,
    pub dependency_changes: Vec<DependencyUpsertChange>,
    pub upgrades_available: usize,
    /// Per-target sync outcomes from the target sync phase.
    pub target_outcomes: Vec<crate::target_sync::TargetSyncOutcome>,
    /// Whether this was a dry run (`--diff`). Affects output wording only.
    pub dry_run: bool,
    /// Native harness agent outputs emitted this run that are new or content-changed
    /// vs the previous lock, as `(target_root, dest_path)`. Surfaced so native
    /// emission is not silent in the summary.
    pub native_emitted: Vec<(String, String)>,
    /// Native harness agent outputs removed this run, as `(target_root, dest_path)`.
    /// Surfaced so SuppressAll / selective prunes are not reported as "up to date".
    pub native_removed: Vec<(String, String)>,
}

impl SyncReport {
    /// Whether the sync produced any unresolved conflicts.
    pub fn has_conflicts(&self) -> bool {
        self.applied
            .outcomes
            .iter()
            .any(|o| matches!(o.action, apply::ActionTaken::Conflicted))
    }
}

/// What a CLI command requests from the sync pipeline.
#[derive(Debug, Clone)]
pub struct SyncRequest {
    /// How to resolve versions.
    pub resolution: ResolutionMode,
    /// Config mutation to apply under flock.
    pub mutation: Option<ConfigMutation>,
    /// Behavior flags.
    pub options: SyncOptions,
    /// Whether lossiness warnings are included in the returned report.
    /// `Surface` for `mars sync` / `mars upgrade`; `Hidden` for validate/export/add/repair.
    pub lossiness_mode: LossinessMode,
}

/// Resolution behavior for the resolver stage.
#[derive(Debug, Clone)]
pub enum ResolutionMode {
    /// Normal sync behavior.
    Normal,
    /// Upgrade behavior (maximize versions), optionally scoped to specific
    /// sources and optionally bumping direct constraints.
    Maximize {
        targets: HashSet<SourceName>,
        bump: bool,
    },
}

// ---------------------------------------------------------------------------
// Pipeline phase structs — typed handoffs between pipeline stages.
// Phase functions consume prior state by value (move semantics, no cloning).
// ---------------------------------------------------------------------------

/// Phase 1: Load and validate configuration under sync lock.
pub(crate) struct LoadedConfig {
    pub config: Config,
    pub local: LocalConfig,
    pub effective: EffectiveConfig,
    pub old_lock: LockFile,
    pub dependency_changes: Vec<DependencyUpsertChange>,
    /// Intentional keepalive — holds the sync file lock for the duration of the pipeline. Dropping this field releases the lock.
    #[allow(dead_code)]
    pub sync_lock: FileLock,
}

/// Phase 2: Resolved dependency graph.
pub(crate) struct ResolvedState {
    pub loaded: LoadedConfig,
    pub graph: ResolvedGraph,
    pub upgrades_available: usize,
}

/// Phase 3: Desired target state after discovery + filtering.
pub(crate) struct TargetedState {
    pub resolved: ResolvedState,
    pub target: TargetState,
    pub warnings: Vec<ValidationWarning>,
}

/// Phase 4: Diff + plan ready for execution.
pub(crate) struct PlannedState {
    pub targeted: TargetedState,
    pub plan: plan::SyncPlan,
}

/// Phase 5: Applied results.
pub(crate) struct AppliedState {
    pub planned: PlannedState,
    pub applied: ApplyResult,
}

/// Phase 6: Target sync results.
pub(crate) struct SyncedState {
    pub applied: AppliedState,
    pub target_outcomes: Vec<crate::target_sync::TargetSyncOutcome>,
    pub config_entries: BTreeMap<String, BTreeMap<String, crate::lock::ConfigEntryRecord>>,
    pub compiled_native_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub removed_native_outputs: Vec<crate::compiler::RemovedNativeOutput>,
}

/// Execute the unified sync pipeline.
///
/// Orchestrates phase functions, each consuming the prior phase's output struct.
pub fn execute(ctx: &MarsContext, request: &SyncRequest) -> Result<SyncReport, MarsError> {
    validate_request(request)?;
    let mut diag = DiagnosticCollector::with_lossiness_mode(request.lossiness_mode);
    let ir = crate::reader::read(ctx, request, &mut diag)?;
    crate::compiler::compile(ctx, ir, request, &mut diag)
}

// ---------------------------------------------------------------------------
// Phase functions
// ---------------------------------------------------------------------------

/// Phase 1: Acquire sync lock, load config, apply mutations, merge effective config,
/// and load the existing lock file.
pub(crate) fn load_config(
    ctx: &MarsContext,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<LoadedConfig, MarsError> {
    let project_root = &ctx.project_root;
    let mars_dir = project_root.join(".mars");

    std::fs::create_dir_all(mars_dir.join("cache"))?;

    // Acquire sync lock before any config reads/mutations.
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    // Load config under lock (auto-init when mutating and missing).
    let mut config = match crate::config::load(project_root) {
        Ok(config) => config,
        Err(err) if mutation::is_config_not_found(&err) && request.mutation.is_some() => Config {
            settings: Settings::default(),
            ..Config::default()
        },
        Err(err) => return Err(err),
    };

    // Apply config mutation.
    let dependency_changes = if let Some(m) = &request.mutation {
        mutation::apply_mutation(&mut config, m)?
    } else {
        Vec::new()
    };

    // Load/mutate local overrides under the same lock.
    let mut local = crate::config::load_local(project_root)?;
    if let Some(m) = &request.mutation {
        mutation::apply_local_mutation(&mut local, m);
    }

    // Build effective config.
    let (effective, config_diagnostics) =
        crate::config::merge_with_root(config.clone(), local.clone(), project_root)?;
    diag.extend(config_diagnostics);

    // Load existing lock file, routing legacy promotion warnings through sync diagnostics.
    let (old_lock, lock_diagnostics) = crate::lock::load_with_diagnostics(project_root)?;
    diag.extend(lock_diagnostics);

    Ok(LoadedConfig {
        config,
        local,
        effective,
        old_lock,
        dependency_changes,
        sync_lock: _sync_lock,
    })
}

/// Phase 2: Validate upgrade targets, resolve the dependency graph.
pub(crate) fn resolve_graph(
    ctx: &MarsContext,
    mut loaded: LoadedConfig,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedState, MarsError> {
    validate_targets(&request.resolution, &loaded.effective)?;

    let cache = GlobalCache::new()?;
    let source_provider = provider::RealSourceProvider::new(&cache, &ctx.project_root);
    let resolve_options = to_resolve_options(&request.resolution, request.options.frozen)
        .with_staging_root(ctx.project_root.join(".mars/staging"));
    let graph = crate::resolve::resolve(
        &loaded.effective,
        &source_provider,
        Some(&loaded.old_lock),
        &resolve_options,
        diag,
    )?;
    let upgrades_available = if request.options.frozen || !request.options.check_upgrades {
        0
    } else {
        upgrades::count_compatible_upgrades(&graph, &source_provider, diag)
    };

    let bump_entries = planned_bump_entries(&loaded.config, &graph, &request.resolution);
    if !bump_entries.is_empty() {
        let bump_changes = mutation::apply_mutation(
            &mut loaded.config,
            &ConfigMutation::BatchUpsert(bump_entries),
        )?;
        loaded.dependency_changes.extend(bump_changes);
    }

    // Merge model config from dependency tree (for diagnostics side effects).
    let _ = crate::models::merged_model_aliases(
        &graph,
        &loaded.effective,
        &loaded.config,
        &loaded.local,
        diag,
    );

    Ok(ResolvedState {
        loaded,
        graph,
        upgrades_available,
    })
}

/// Phase 3: Build target state, handle collisions, rewrite frontmatter refs, validate.
///
/// `local_items` are pre-discovered by the reader stage; no discovery is
/// performed here so that dest-path assignment remains the only compiler
/// concern for local content.
pub(crate) fn build_target(
    ctx: &MarsContext,
    resolved: ResolvedState,
    local_items: Vec<crate::local_source::LocalDiscoveredItem>,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<TargetedState, MarsError> {
    // Use .mars/ as the canonical content root for diff/collision checks.
    let mars_dir = ctx.project_root.join(".mars");
    let managed_root = &mars_dir;

    // Build target state from resolved graph.
    let (mut target_state, renames, collision_renames) =
        target::build_with_collisions_and_diag(&resolved.graph, &resolved.loaded.effective, diag)?;

    let local_source_name: SourceName = SourceOrigin::LocalPackage.to_string().into();
    let local_source_id = SourceId::Path {
        canonical: dunce::canonicalize(&ctx.project_root)
            .unwrap_or_else(|_| ctx.project_root.clone()),
        subpath: None,
    };
    let old_lock_index = LockIndex::new(&resolved.loaded.old_lock);

    for item in local_items {
        let staging_root = ctx.project_root.join(".mars/staging");
        let item_key = format!("{}:{}", item.discovered.id.kind, item.discovered.id.name);
        let staged_path = crate::staging::stage_local_item(
            &item.disk_path(),
            item.discovered.id.kind,
            crate::dialect::Dialect::resolve_local(None, &item.root),
            &resolved.loaded.effective.skills,
            &staging_root,
            &item_key,
            (item.discovered.id.kind == ItemKind::Skill).then(|| item.discovered.id.name.as_str()),
            diag,
        )?;
        let source_path = staged_path;
        let is_flat_skill = item.discovered.id.kind == ItemKind::Skill
            && item.discovered.source_path == Path::new(".");
        let source_hash = if is_flat_skill {
            ContentHash::from(hash::compute_skill_hash_filtered(
                &source_path,
                crate::fs::FLAT_SKILL_EXCLUDED_TOP_LEVEL,
            )?)
        } else {
            ContentHash::from(hash::compute_hash(&source_path, item.discovered.id.kind)?)
        };
        if item.discovered.id.kind == ItemKind::Agent
            && let Err(message) =
                crate::target::validate_agent_filename(item.discovered.id.name.as_str())
        {
            diag.error_with_category(
                "invalid-agent-filename",
                format!("{message}; skipping local agent"),
                crate::diagnostic::DiagnosticCategory::Validation,
            );
            continue;
        }
        let dest_path =
            default_dest_path(item.discovered.id.kind, item.discovered.id.name.as_str());

        if let Some(existing) = target_state.items.shift_remove(&dest_path)
            && existing.source_hash != source_hash
        {
            diag.warn(
                "local-shadow",
                format!(
                    "local {} `{}` shadows dependency `{}` {} `{}`",
                    item.discovered.id.kind,
                    item.discovered.id.name,
                    existing.source_name,
                    existing.id.kind,
                    existing.id.name
                ),
            );
        }

        let disk_path = dest_path.resolve(managed_root);
        if !old_lock_index.contains_output(CANONICAL_TARGET_ROOT, &dest_path)
            && disk_path.symlink_metadata().is_ok()
        {
            diag.warn(
                "unmanaged-collision",
                format!(
                    "local {} `{}` collides with unmanaged path `{}` — leaving existing content untouched",
                    item.discovered.id.kind, item.discovered.id.name, dest_path
                ),
            );
            continue;
        }

        target_state.items.insert(
            dest_path.clone(),
            TargetItem {
                id: ItemId {
                    kind: item.discovered.id.kind,
                    name: item.discovered.id.name.clone(),
                },
                source_name: local_source_name.clone(),
                origin: SourceOrigin::LocalPackage,
                source_id: local_source_id.clone(),
                source_path,
                dest_path,
                source_hash,
                is_flat_skill,
                rewritten_content: None,
            },
        );
    }

    // Prevent managed installs from overwriting unmanaged files.
    let unmanaged_collisions = target::check_unmanaged_collisions(
        managed_root,
        &resolved.loaded.old_lock,
        &target_state,
        request.options.force,
    );
    for collision in &unmanaged_collisions {
        diag.warn(
            "unmanaged-collision",
            format!(
                "source `{}` collides with unmanaged path `{}` — leaving existing content untouched",
                collision.source_name, collision.path
            ),
        );
        target_state.items.shift_remove(&collision.path);
    }

    // Rewrite frontmatter refs against the post-prune target state.
    let rename_index = rewrite::RenameIndex::new(&renames, &collision_renames, &target_state);
    if !rename_index.is_empty() {
        let dep_precedence: Vec<SourceName> = resolved
            .loaded
            .effective
            .dependencies
            .keys()
            .cloned()
            .collect();
        let rewrite_warnings = rewrite::apply_renames(
            &mut target_state,
            &rename_index,
            &resolved.graph,
            &dep_precedence,
        )?;
        for w in &rewrite_warnings {
            diag.warn("rewrite-warning", w.to_string());
        }
    }

    validate::warn_config_dangles_after_rename(
        &renames,
        &collision_renames,
        &target_state,
        &resolved.loaded,
        diag,
    );

    validate::validate_skill_frontmatter_in_target(&target_state, diag);

    // Validate skill references.
    let warnings = validate::validate_skill_refs(&target_state);

    Ok(TargetedState {
        resolved,
        target: target_state,
        warnings,
    })
}

/// Phase 4: Compute diff, create plan.
pub(crate) fn create_plan(
    ctx: &MarsContext,
    targeted: TargetedState,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<PlannedState, MarsError> {
    // Diff against .mars/ canonical store.
    let mars_dir = ctx.project_root.join(".mars");
    let managed_root = &mars_dir;
    let cache_bases_dir = mars_dir.join("cache").join("bases");

    // Compute diff.
    let sync_diff = diff::compute(
        managed_root,
        &targeted.resolved.loaded.old_lock,
        &targeted.target,
        request.options.force,
    )?;

    if !request.options.force {
        for entry in &sync_diff.items {
            if let diff::DiffEntry::LocalModified { target, .. } = entry {
                diag.warn(
                    "disk-lock-divergent",
                    format!(
                        "{} diverged from mars.lock checksum; preserving local content (run `{cmd1}` or `{cmd2}` to reset)",
                        target.dest_path,
                        cmd1 = managed_cmd("mars sync --force"),
                        cmd2 = managed_cmd("mars repair"),
                    ),
                );
            }
        }
    }

    // Create plan.
    let sync_plan = plan::create(&sync_diff, &request.options, &cache_bases_dir, diag);

    Ok(PlannedState {
        targeted,
        plan: sync_plan,
    })
}

/// Check that a frozen sync has no pending changes.
pub(crate) fn check_frozen_gate(planned: &PlannedState) -> Result<(), MarsError> {
    let has_changes = planned.plan.actions.iter().any(|a| {
        !matches!(
            a,
            plan::PlannedAction::Skip { .. } | plan::PlannedAction::KeepLocal { .. }
        )
    });
    if has_changes {
        return Err(MarsError::FrozenViolation {
            message: "lock file would change but --frozen is set".into(),
        });
    }
    Ok(())
}

/// Phase 5: Persist config if mutated, apply plan to .mars/ canonical store.
pub(crate) fn apply_plan(
    ctx: &MarsContext,
    planned: PlannedState,
    request: &SyncRequest,
) -> Result<AppliedState, MarsError> {
    let project_root = &ctx.project_root;
    let mars_dir = project_root.join(".mars");
    let cache_bases_dir = mars_dir.join("cache").join("bases");

    let has_bump_version_changes =
        has_version_changes(&planned.targeted.resolved.loaded.dependency_changes)
            && matches!(
                request.resolution,
                ResolutionMode::Maximize { bump: true, .. }
            );
    let has_mutation = request.mutation.is_some() || has_bump_version_changes;

    // Persist config/local only after validation gate and before apply.
    if has_mutation && !request.options.dry_run {
        match &request.mutation {
            Some(ConfigMutation::SetOverride { .. } | ConfigMutation::ClearOverride { .. }) => {
                crate::config::save_local(project_root, &planned.targeted.resolved.loaded.local)?;
            }
            Some(
                ConfigMutation::UpsertDependency { .. }
                | ConfigMutation::BatchUpsert(..)
                | ConfigMutation::RemoveDependency { .. }
                | ConfigMutation::SetRename { .. },
            ) => {
                crate::config::save(project_root, &planned.targeted.resolved.loaded.config)?;
            }
            None => {
                if has_bump_version_changes {
                    crate::config::save(project_root, &planned.targeted.resolved.loaded.config)?;
                }
            }
        }
    }

    // Apply plan to .mars/ canonical store (D25).
    // Content is written to .mars/agents/ and .mars/skills/, then
    // sync_targets() copies to all managed target directories.
    let applied = apply::execute(&mars_dir, &planned.plan, &request.options, &cache_bases_dir)?;

    Ok(AppliedState { planned, applied })
}

/// Phase 6: Sync managed targets from .mars/ canonical store.
///
/// Copies content from .mars/ to all configured target directories.
/// Non-fatal — target sync errors are recorded as diagnostics.
/// Lock is written regardless of target sync outcome (D21).
pub(crate) fn sync_targets(
    ctx: &MarsContext,
    applied: AppliedState,
    request: &SyncRequest,
    agent_surface_policy: crate::compiler::AgentSurfacePolicy,
    diag: &mut DiagnosticCollector,
) -> SyncedState {
    if request.options.dry_run {
        return SyncedState {
            applied,
            target_outcomes: Vec::new(),
            config_entries: BTreeMap::new(),
            compiled_native_outputs: Vec::new(),
            removed_native_outputs: Vec::new(),
        };
    }

    let mars_dir = ctx.project_root.join(".mars");
    let targets = applied
        .planned
        .targeted
        .resolved
        .loaded
        .effective
        .settings
        .managed_targets();
    let old_lock = &applied.planned.targeted.resolved.loaded.old_lock;

    let filtered_outcomes;
    let orphan_preserve_paths;
    let (target_outcomes_source, orphan_preserve) = match &agent_surface_policy {
        crate::compiler::AgentSurfacePolicy::SuppressAll => {
            filtered_outcomes = crate::compiler::suppress_agent_outcomes(&applied.applied.outcomes);
            (&filtered_outcomes, None)
        }
        crate::compiler::AgentSurfacePolicy::EmitSelective(spec) => {
            orphan_preserve_paths =
                crate::compiler::selective_native_orphan_preserve_paths(old_lock, spec);
            filtered_outcomes = crate::compiler::omit_agent_outcomes(&applied.applied.outcomes);
            (&filtered_outcomes, Some(&orphan_preserve_paths))
        }
        crate::compiler::AgentSurfacePolicy::EmitAll => (&applied.applied.outcomes, None),
    };

    let target_sync_ctx = crate::target_sync::TargetSyncContext {
        old_lock,
        force: request.options.force,
        collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
        orphan_preserve_paths: orphan_preserve,
    };
    let target_outcomes = crate::target_sync::sync_managed_targets(
        &ctx.project_root,
        &mars_dir,
        &targets,
        target_outcomes_source,
        &target_sync_ctx,
        diag,
    );

    SyncedState {
        applied,
        target_outcomes,
        config_entries: BTreeMap::new(),
        compiled_native_outputs: Vec::new(),
        removed_native_outputs: Vec::new(),
    }
}

/// Phase 7: Write lock file, construct SyncReport.
///
/// Lock is written regardless of target sync outcome (D21).
pub(crate) fn finalize(
    ctx: &MarsContext,
    state: SyncedState,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<SyncReport, MarsError> {
    let project_root = &ctx.project_root;
    let old_lock = &state.applied.planned.targeted.resolved.loaded.old_lock;
    let graph = &state.applied.planned.targeted.resolved.graph;
    // Native-agent surface deltas for the summary: removals are unambiguous; emits
    // are filtered to new/changed outputs so steady-state re-emits stay quiet.
    let native_removed: Vec<(String, String)> = state.removed_native_outputs.clone();
    let native_emitted: Vec<(String, String)> = state
        .compiled_native_outputs
        .iter()
        .filter(|out| crate::lock::native_output_is_new_or_changed(old_lock, out))
        .map(|out| (out.target_root.clone(), out.dest_path.clone()))
        .collect();

    // Write lock file (D21 — regardless of target sync outcome).
    if !request.options.dry_run {
        let dep_models = crate::models::declaration_ordered_dep_models(
            graph,
            &state.applied.planned.targeted.resolved.loaded.effective,
        );
        let mut dep_model_aliases = crate::models::dependency_alias_snapshot(&dep_models);
        dep_model_aliases.sort_keys();

        let mut new_lock = crate::lock::build(
            graph,
            &state.applied.applied,
            old_lock,
            state.config_entries,
        )?;
        new_lock.dependency_model_aliases = dep_model_aliases;
        crate::lock::apply_target_sync_outputs(&mut new_lock, &state.target_outcomes);
        crate::lock::apply_removed_native_outputs(&mut new_lock, &state.removed_native_outputs);
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.compiled_native_outputs);
        if let Some(warning) =
            crate::compiler::persist_lock_then_native_agent_manifest(project_root, &new_lock)?
        {
            diag.warn("native-agent-manifest-write", warning);
        }

        // Best-effort models cache refresh: ensure the catalog covers any
        // new aliases we're about to persist. Sync never aborts on refresh
        // failure — warn and continue.
        let mars_path = ctx.project_root.join(".mars");
        let ttl = state
            .applied
            .planned
            .targeted
            .resolved
            .loaded
            .effective
            .settings
            .models_cache_ttl_hours;
        let refresh = crate::models::resolve_models_refresh_control(
            request.options.refresh_models,
            request.options.no_refresh_models,
        )?;
        match crate::models::ensure_fresh(&mars_path, ttl, refresh.catalog_mode) {
            Ok((_, crate::models::RefreshOutcome::StaleFallback { reason })) => {
                diag.warn(
                    "models-cache-refresh",
                    format!("using stale models cache: {reason}"),
                );
            }
            Ok((_, crate::models::RefreshOutcome::Offline)) => {}
            Ok(_) => {}
            Err(err) => {
                diag.warn(
                    "models-cache-refresh",
                    format!("failed to refresh models cache: {err}"),
                );
            }
        }
    }

    for w in &state.applied.planned.targeted.warnings {
        match w {
            ValidationWarning::MissingSkill {
                agent,
                skill_name,
                suggestion,
            } => {
                let msg = match suggestion {
                    Some(s) => format!(
                        "agent `{}` references missing skill `{}` (did you mean `{}`?)",
                        agent.name, skill_name, s
                    ),
                    None => {
                        format!(
                            "agent `{}` references missing skill `{}`",
                            agent.name, skill_name
                        )
                    }
                };
                diag.warn("missing-skill", msg);
            }
        }
    }
    let dependency_changes = state
        .applied
        .planned
        .targeted
        .resolved
        .loaded
        .dependency_changes;
    let upgrades_available = state.applied.planned.targeted.resolved.upgrades_available;

    let diagnostics = diag.drain();

    Ok(SyncReport {
        applied: state.applied.applied,
        pruned: Vec::new(),
        diagnostics,
        dependency_changes,
        upgrades_available,
        target_outcomes: state.target_outcomes,
        dry_run: request.options.dry_run,
        native_emitted,
        native_removed,
    })
}

fn default_dest_path(kind: ItemKind, name: &str) -> DestPath {
    match kind {
        ItemKind::Agent => DestPath::from(format!("agents/{name}.md")),
        ItemKind::Skill => DestPath::from(format!("skills/{name}")),
        ItemKind::Hook => DestPath::from(format!("hooks/{name}")),
        ItemKind::McpServer => DestPath::from(format!("mcp/{name}")),
        ItemKind::BootstrapDoc => DestPath::from(format!("bootstrap/{name}/BOOTSTRAP.md")),
    }
}

fn validate_request(request: &SyncRequest) -> Result<(), MarsError> {
    if request.options.frozen && matches!(request.resolution, ResolutionMode::Maximize { .. }) {
        return Err(MarsError::InvalidRequest {
            message:
                "cannot use --frozen with upgrade (frozen locks versions; upgrade maximizes them)"
                    .to_string(),
        });
    }

    if request.options.frozen && request.mutation.is_some() {
        return Err(MarsError::InvalidRequest {
            message:
                "cannot modify config in --frozen mode (config change would require lock update)"
                    .to_string(),
        });
    }

    Ok(())
}

fn validate_targets(
    resolution: &ResolutionMode,
    effective: &EffectiveConfig,
) -> Result<(), MarsError> {
    if let ResolutionMode::Maximize { targets, .. } = resolution {
        for name in targets {
            if !effective.dependencies.contains_key(name) {
                return Err(MarsError::Source {
                    source_name: name.to_string(),
                    message: format!("dependency `{name}` not found in mars.toml"),
                });
            }
        }
    }

    Ok(())
}

fn to_resolve_options(mode: &ResolutionMode, frozen: bool) -> ResolveOptions {
    if frozen {
        return ResolveOptions::frozen();
    }

    match mode {
        ResolutionMode::Normal => ResolveOptions::sync(),
        ResolutionMode::Maximize { targets, bump } => {
            ResolveOptions::upgrade(targets.clone(), *bump)
        }
    }
}

fn planned_bump_entries(
    config: &Config,
    graph: &ResolvedGraph,
    mode: &ResolutionMode,
) -> Vec<(SourceName, crate::config::DependencyEntry)> {
    let ResolutionMode::Maximize {
        targets,
        bump: true,
    } = mode
    else {
        return Vec::new();
    };

    config
        .dependencies
        .iter()
        .filter_map(|(name, entry)| {
            if !targets.is_empty() && !targets.contains(name) {
                return None;
            }
            // Only git dependencies with semver-tagged resolution can be bumped.
            entry.url.as_ref()?;
            let node = graph.nodes.get(name)?;
            let resolved_version = node.resolved_ref.version.as_ref()?;
            let resolved_tag = node.resolved_ref.version_tag.as_ref()?;
            if !constraint_needs_bump(entry.version.as_deref(), resolved_version) {
                return None;
            }
            if entry.version.as_deref() == Some(resolved_tag.as_str()) {
                return None;
            }
            let mut bumped = entry.clone();
            bumped.version = Some(resolved_tag.clone());
            Some((name.clone(), bumped))
        })
        .collect()
}

fn constraint_needs_bump(current: Option<&str>, resolved: &semver::Version) -> bool {
    match crate::resolve::parse_version_constraint(current) {
        crate::resolve::VersionConstraint::Semver(req) => !req.matches(resolved),
        crate::resolve::VersionConstraint::Latest
        | crate::resolve::VersionConstraint::RefPin(_) => false,
    }
}

fn has_version_changes(changes: &[DependencyUpsertChange]) -> bool {
    changes
        .iter()
        .any(|change| change.old_version != change.new_version)
}

#[cfg(test)]
mod tests;
