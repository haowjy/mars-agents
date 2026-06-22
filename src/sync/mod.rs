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

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use crate::config::{Config, EffectiveConfig, LocalConfig, Settings};
use crate::diagnostic::{Diagnostic, DiagnosticCollector};
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
    let mut diag = DiagnosticCollector::new();
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
    let (mut target_state, renames) =
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
            (item.discovered.id.kind == ItemKind::Skill)
                .then(|| item.discovered.id.name.as_str()),
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

    // Handle collisions + rewrite frontmatter refs.
    if !renames.is_empty() {
        let rewrite_warnings =
            target::rewrite_skill_refs(&mut target_state, &renames, &resolved.graph)?;
        for w in &rewrite_warnings {
            diag.warn("rewrite-warning", w.to_string());
        }
    }

    validate_skill_frontmatter_in_target(&target_state, diag);

    // Validate skill references.
    let warnings = validate_skill_refs(&target_state);

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

    Ok(SyncReport {
        applied: state.applied.applied,
        pruned: Vec::new(),
        diagnostics: diag.drain(),
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

/// Validate skill references: check that agents' `skills:` frontmatter entries
/// reference skills that exist in the target state.
fn validate_skill_refs(target: &target::TargetState) -> Vec<ValidationWarning> {
    use crate::lock::ItemKind;
    use crate::validate::{extract_skills_from_content, find_suggestion};

    // Collect available skill names
    let available_skills: HashSet<String> = target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| item.id.name.to_string())
        .collect();

    let mut warnings = Vec::new();

    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Agent)
    {
        let content = match &item.rewritten_content {
            Some(content) => content.clone(),
            None => std::fs::read_to_string(&item.source_path).unwrap_or_default(),
        };
        for skill_name in extract_skills_from_content(&content) {
            if !available_skills.contains(&skill_name) {
                let suggestion = find_suggestion(&skill_name, &available_skills);
                warnings.push(ValidationWarning::MissingSkill {
                    agent: item.id.clone(),
                    skill_name,
                    suggestion,
                });
            }
        }
    }

    warnings
}

fn validate_skill_frontmatter_in_target(
    target: &target::TargetState,
    diag: &mut DiagnosticCollector,
) {
    use crate::lock::ItemKind;

    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
    {
        validate_skill_frontmatter_at_source(&item.source_path, item.id.name.as_str(), diag);
    }
}

fn validate_skill_frontmatter_at_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    let skill_md = if source_path.is_dir() {
        source_path.join("SKILL.md")
    } else {
        source_path.to_path_buf()
    };
    let Ok(content) = std::fs::read_to_string(&skill_md) else {
        return;
    };
    let mut skill_diags = Vec::new();
    let _ = crate::compiler::skills::parse_skill_content(&content, &mut skill_diags);
    for d in skill_diags {
        if d.is_error() {
            diag.error_with_category(
                "skill-schema-error",
                format!("skill `{skill_name}`: {}", d.message()),
                crate::diagnostic::DiagnosticCategory::Validation,
            );
        } else {
            diag.warn(
                "skill-schema-warning",
                format!("skill `{skill_name}`: {}", d.message()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::lock::{ItemKind, LockFile};
    use crate::resolve::{ResolvedGraph, ResolvedNode};
    use indexmap::IndexMap;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper to set up a complete sync context with temp dirs.
    struct TestFixture {
        project_root: TempDir,
        managed_root: PathBuf,
        source_trees: Vec<TempDir>,
    }

    impl TestFixture {
        fn new() -> Self {
            let project_root = TempDir::new().unwrap();
            let managed_root = project_root.path().join(".agents");
            // Create .mars/cache directories
            fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
            TestFixture {
                project_root,
                managed_root,
                source_trees: Vec::new(),
            }
        }

        fn add_source(&mut self, agents: &[(&str, &str)], skills: &[(&str, &str)]) -> usize {
            let dir = TempDir::new().unwrap();
            if !agents.is_empty() {
                let agents_dir = dir.path().join("agents");
                fs::create_dir_all(&agents_dir).unwrap();
                for (name, content) in agents {
                    fs::write(agents_dir.join(name), content).unwrap();
                }
            }
            if !skills.is_empty() {
                let skills_dir = dir.path().join("skills");
                fs::create_dir_all(&skills_dir).unwrap();
                for (name, content) in skills {
                    let skill_dir = skills_dir.join(name);
                    fs::create_dir_all(&skill_dir).unwrap();
                    fs::write(skill_dir.join("SKILL.md"), content).unwrap();
                }
            }
            self.source_trees.push(dir);
            self.source_trees.len() - 1
        }

        fn project_root(&self) -> &std::path::Path {
            self.project_root.path()
        }

        fn managed_root(&self) -> &std::path::Path {
            &self.managed_root
        }

        fn tree_path(&self, idx: usize) -> PathBuf {
            self.source_trees[idx].path().to_path_buf()
        }
    }

    fn make_graph_config(
        fixture: &TestFixture,
        sources: Vec<(&str, usize, FilterMode)>,
    ) -> (ResolvedGraph, EffectiveConfig) {
        let mut nodes = IndexMap::new();
        let mut order = Vec::new();
        let mut config_dependencies = IndexMap::new();

        for (name, tree_idx, filter) in sources {
            let tree_path = fixture.tree_path(tree_idx);
            nodes.insert(
                name.into(),
                ResolvedNode {
                    source_name: name.into(),
                    source_id: crate::types::SourceId::Path {
                        canonical: tree_path.clone(),
                        subpath: None,
                    },
                    rooted_ref: crate::resolve::RootedSourceRef {
                        checkout_root: tree_path.clone(),
                        package_root: tree_path.clone(),
                    },
                    resolved_ref: crate::source::ResolvedRef {
                        source_name: name.into(),
                        version: None,
                        version_tag: None,
                        commit: None,
                        tree_path: tree_path.clone(),
                    },
                    latest_version: None,
                    manifest: None,
                    deps: vec![],
                },
            );
            order.push(name.into());

            config_dependencies.insert(
                name.into(),
                EffectiveDependency {
                    name: name.into(),
                    id: crate::types::SourceId::Path {
                        canonical: tree_path.clone(),
                        subpath: None,
                    },
                    spec: SourceSpec::Path(tree_path),
                    subpath: None,
                    filter,
                    rename: crate::types::RenameMap::new(),
                    dialect: None,
                    is_overridden: false,
                    original_git: None,
                },
            );
        }

        (
            ResolvedGraph {
                nodes,
                order,
                filters: std::collections::HashMap::new(),
                version_constraints: std::collections::HashMap::new(),
            },
            EffectiveConfig {
                dependencies: config_dependencies,
                settings: Settings::default(),
        skills: indexmap::IndexMap::new(),
            },
        )
    }

    fn path_dependency_entry(path: &std::path::Path) -> DependencyEntry {
        DependencyEntry {
            url: None,
            path: Some(path.to_path_buf()),
            subpath: None,
            version: None,
            dialect: None,
            filter: FilterConfig::default(),
        }
    }

    fn git_dependency_entry(url: &str, version: &str, filter: FilterConfig) -> DependencyEntry {
        DependencyEntry {
            url: Some(url.into()),
            path: None,
            subpath: None,
            version: Some(version.to_string()),
            dialect: None,
            filter,
        }
    }

    fn create_sync_plan(
        sync_diff: &diff::SyncDiff,
        options: &SyncOptions,
        cache_bases_dir: &std::path::Path,
    ) -> plan::SyncPlan {
        let mut diag = DiagnosticCollector::new();
        plan::create(sync_diff, options, cache_bases_dir, &mut diag)
    }

    fn graph_with_versions(entries: &[(&str, &str, &str)]) -> ResolvedGraph {
        let mut nodes = IndexMap::new();
        let mut order = Vec::new();
        for (name, url, tag) in entries {
            let version = semver::Version::parse(tag.trim_start_matches('v')).unwrap();
            nodes.insert(
                (*name).into(),
                ResolvedNode {
                    source_name: (*name).into(),
                    source_id: crate::types::SourceId::git(crate::types::SourceUrl::from(*url)),
                    rooted_ref: crate::resolve::RootedSourceRef {
                        checkout_root: PathBuf::from(format!("/tmp/{name}")),
                        package_root: PathBuf::from(format!("/tmp/{name}")),
                    },
                    resolved_ref: crate::source::ResolvedRef {
                        source_name: (*name).into(),
                        version: Some(version),
                        version_tag: Some((*tag).to_string()),
                        commit: Some("abc123".into()),
                        tree_path: PathBuf::from(format!("/tmp/{name}")),
                    },
                    latest_version: None,
                    manifest: None,
                    deps: vec![],
                },
            );
            order.push((*name).into());
        }

        ResolvedGraph {
            nodes,
            order,
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn validate_request_rejects_frozen_with_maximize() {
        let request = SyncRequest {
            resolution: ResolutionMode::Maximize {
                targets: HashSet::new(),
                bump: false,
            },
            mutation: None,
            options: SyncOptions {
                frozen: true,
                ..SyncOptions::default()
            },
        };

        let err = validate_request(&request).unwrap_err();
        assert!(matches!(err, MarsError::InvalidRequest { .. }));
        assert!(err.to_string().contains("--frozen"));
    }

    #[test]
    fn validate_request_rejects_frozen_with_mutation() {
        let request = SyncRequest {
            resolution: ResolutionMode::Normal,
            mutation: Some(ConfigMutation::RemoveDependency {
                name: "base".into(),
            }),
            options: SyncOptions {
                frozen: true,
                ..SyncOptions::default()
            },
        };

        let err = validate_request(&request).unwrap_err();
        assert!(matches!(err, MarsError::InvalidRequest { .. }));
        assert!(err.to_string().contains("cannot modify config"));
    }

    #[test]
    fn planned_bump_entries_bump_all_outdated_pins() {
        let mut config = Config::default();
        config.dependencies.insert(
            "base".into(),
            git_dependency_entry(
                "https://example.com/base.git",
                "v1.0.0",
                FilterConfig::default(),
            ),
        );
        config.dependencies.insert(
            "tools".into(),
            git_dependency_entry(
                "https://example.com/tools.git",
                "v2.0.0",
                FilterConfig::default(),
            ),
        );
        config.dependencies.insert(
            "floating".into(),
            DependencyEntry {
                url: Some("https://example.com/floating.git".into()),
                path: None,
                subpath: None,
                version: None,
            dialect: None,
                filter: FilterConfig::default(),
            },
        );

        let graph = graph_with_versions(&[
            ("base", "https://example.com/base.git", "v1.2.0"),
            ("tools", "https://example.com/tools.git", "v2.0.0"),
            ("floating", "https://example.com/floating.git", "v3.0.0"),
        ]);

        let mode = ResolutionMode::Maximize {
            targets: HashSet::new(),
            bump: true,
        };
        let entries = planned_bump_entries(&config, &graph, &mode);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, SourceName::from("base"));
        assert_eq!(entries[0].1.version.as_deref(), Some("v1.2.0"));
    }

    #[test]
    fn planned_bump_entries_bump_specific_targets_only() {
        let mut config = Config::default();
        config.dependencies.insert(
            "base".into(),
            git_dependency_entry(
                "https://example.com/base.git",
                "v1.0.0",
                FilterConfig::default(),
            ),
        );
        config.dependencies.insert(
            "tools".into(),
            git_dependency_entry(
                "https://example.com/tools.git",
                "v1.0.0",
                FilterConfig::default(),
            ),
        );

        let graph = graph_with_versions(&[
            ("base", "https://example.com/base.git", "v2.0.0"),
            ("tools", "https://example.com/tools.git", "v2.0.0"),
        ]);

        let mode = ResolutionMode::Maximize {
            targets: HashSet::from([SourceName::from("tools")]),
            bump: true,
        };
        let entries = planned_bump_entries(&config, &graph, &mode);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, SourceName::from("tools"));
        assert_eq!(entries[0].1.version.as_deref(), Some("v2.0.0"));
    }

    #[test]
    fn planned_bump_entries_noop_when_already_latest() {
        let mut config = Config::default();
        config.dependencies.insert(
            "base".into(),
            git_dependency_entry(
                "https://example.com/base.git",
                "v1.2.0",
                FilterConfig::default(),
            ),
        );

        let graph = graph_with_versions(&[("base", "https://example.com/base.git", "v1.2.0")]);

        let mode = ResolutionMode::Maximize {
            targets: HashSet::new(),
            bump: true,
        };
        let entries = planned_bump_entries(&config, &graph, &mode);
        assert!(entries.is_empty());
    }

    #[test]
    fn planned_bump_entries_preserve_filters_and_renames() {
        let mut rename = crate::types::RenameMap::new();
        rename.insert("coder".into(), "coder-v2".into());

        let mut config = Config::default();
        config.dependencies.insert(
            "base".into(),
            git_dependency_entry(
                "https://example.com/base.git",
                "v1.0.0",
                FilterConfig {
                    agents: Some(vec!["coder".into()]),
                    rename: Some(rename.clone()),
                    ..FilterConfig::default()
                },
            ),
        );

        let graph = graph_with_versions(&[("base", "https://example.com/base.git", "v2.0.0")]);
        let mode = ResolutionMode::Maximize {
            targets: HashSet::new(),
            bump: true,
        };
        let entries = planned_bump_entries(&config, &graph, &mode);
        let mut mutated = config.clone();
        let changes =
            mutation::apply_mutation(&mut mutated, &ConfigMutation::BatchUpsert(entries)).unwrap();

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old_version.as_deref(), Some("v1.0.0"));
        assert_eq!(changes[0].new_version.as_deref(), Some("v2.0.0"));

        let dep = &mutated.dependencies["base"];
        assert_eq!(dep.version.as_deref(), Some("v2.0.0"));
        assert_eq!(dep.filter.agents.as_deref(), Some(&["coder".into()][..]));
        assert_eq!(dep.filter.rename.as_ref(), Some(&rename));
    }

    #[test]
    fn execute_auto_inits_config_for_mutation() {
        let project_root = TempDir::new().unwrap();
        let managed_root = project_root.path().join(".agents");
        fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
        let source = TempDir::new().unwrap();
        fs::create_dir_all(source.path().join("agents")).unwrap();
        fs::write(source.path().join("agents/coder.md"), "# Coder").unwrap();

        let request = SyncRequest {
            resolution: ResolutionMode::Normal,
            mutation: Some(ConfigMutation::UpsertDependency {
                name: "base".into(),
                entry: path_dependency_entry(source.path()),
            }),
            options: SyncOptions::default(),
        };

        let ctx = MarsContext::for_test(project_root.path().to_path_buf(), managed_root.clone());
        let report = execute(&ctx, &request).unwrap();
        assert!(!report.applied.outcomes.is_empty());
        assert!(project_root.path().join("mars.toml").exists());

        let saved = crate::config::load(project_root.path()).unwrap();
        assert!(saved.dependencies.contains_key("base"));
    }

    #[test]
    fn execute_dry_run_with_mutation_does_not_write_config() {
        let project_root = TempDir::new().unwrap();
        let managed_root = project_root.path().join(".agents");
        fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
        crate::config::save(
            project_root.path(),
            &Config {
                dependencies: IndexMap::new(),
                settings: Settings::default(),
                ..Config::default()
            },
        )
        .unwrap();

        let source = TempDir::new().unwrap();
        fs::create_dir_all(source.path().join("agents")).unwrap();
        fs::write(source.path().join("agents/coder.md"), "# Coder").unwrap();

        let request = SyncRequest {
            resolution: ResolutionMode::Normal,
            mutation: Some(ConfigMutation::UpsertDependency {
                name: "base".into(),
                entry: path_dependency_entry(source.path()),
            }),
            options: SyncOptions {
                dry_run: true,
                ..SyncOptions::default()
            },
        };

        let ctx = MarsContext::for_test(project_root.path().to_path_buf(), managed_root.clone());
        let report = execute(&ctx, &request).unwrap();
        assert!(!report.applied.outcomes.is_empty());

        let saved = crate::config::load(project_root.path()).unwrap();
        assert!(!saved.dependencies.contains_key("base"));
        assert!(!managed_root.join("agents/coder.md").exists());
        assert!(!project_root.path().join("mars.lock").exists());
    }

    // === Integration tests for the pipeline stages ===

    #[test]
    fn full_pipeline_fresh_sync() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(
            &[("coder.md", "# Coder agent")],
            &[("planning", "# Planning skill")],
        );

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // Build target
        let (target, renames) = target::build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2);

        // Compute diff against empty lock
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();

        // All items should be Add
        assert_eq!(sync_diff.items.len(), 2);
        for entry in &sync_diff.items {
            assert!(matches!(entry, diff::DiffEntry::Add { .. }));
        }

        // Create plan
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        assert_eq!(sync_plan.actions.len(), 2);
        for action in &sync_plan.actions {
            assert!(matches!(action, plan::PlannedAction::Install { .. }));
        }

        // Execute plan
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        assert_eq!(result.outcomes.len(), 2);

        // Verify files were created
        assert!(fixture.managed_root().join("agents/coder.md").exists());
        assert!(
            fixture
                .managed_root()
                .join("skills/planning/SKILL.md")
                .exists()
        );

        // Build lock
        let new_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();
        assert_eq!(new_lock.items.len(), 2);
        assert!(new_lock.items.contains_key("agent/coder"));
        assert!(new_lock.items.contains_key("skill/planning"));
    }

    #[test]
    fn re_sync_no_changes() {
        let mut fixture = TestFixture::new();
        let content = "# Coder agent";
        let src_idx = fixture.add_source(&[("coder.md", content)], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // First sync
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        // Second sync with same content
        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

        // All items should be Unchanged
        for entry in &sync_diff2.items {
            assert!(
                matches!(entry, diff::DiffEntry::Unchanged { .. }),
                "expected Unchanged, got {entry:?}"
            );
        }

        let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
        for action in &sync_plan2.actions {
            assert!(matches!(action, plan::PlannedAction::Skip { .. }));
        }
    }

    #[test]
    fn explicit_dialect_change_triggers_update() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(
            &[],
            &[(
                "planning",
                "---\nname: planning\ndescription: d\ndisable-model-invocation: true\n---\n# Planning\n",
            )],
        );
        let tree_path = fixture.tree_path(src_idx);
        let staging_root = fixture.project_root().join(".mars/staging");
        fs::create_dir_all(&staging_root).unwrap();

        let stage = |dialect: crate::dialect::Dialect| {
            crate::staging::stage_rooted_source(
                &"base".into(),
                crate::resolve::RootedSourceRef {
                    checkout_root: tree_path.clone(),
                    package_root: tree_path.clone(),
                },
                dialect,
                &indexmap::IndexMap::new(),
                &crate::types::RenameMap::new(),
                &staging_root,
            )
            .unwrap()
        };

        let mut config = EffectiveConfig {
            dependencies: indexmap::IndexMap::from([(
                "base".into(),
                EffectiveDependency {
                    name: "base".into(),
                    id: crate::types::SourceId::Path {
                        canonical: tree_path.clone(),
                        subpath: None,
                    },
                    spec: SourceSpec::Path(tree_path.clone()),
                    subpath: None,
                    filter: FilterMode::All,
                    rename: crate::types::RenameMap::new(),
                    dialect: Some(crate::dialect::Dialect::MarsNative),
                    is_overridden: false,
                    original_git: None,
                },
            )]),
            settings: Settings::default(),
            skills: indexmap::IndexMap::new(),
        };

        let mut graph = {
            let (mut g, _) =
                make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
            g.nodes.get_mut("base").unwrap().rooted_ref = stage(crate::dialect::Dialect::MarsNative);
            g
        };

        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        graph.nodes.get_mut("base").unwrap().rooted_ref = stage(crate::dialect::Dialect::Claude);
        config.dependencies.get_mut("base").unwrap().dialect =
            Some(crate::dialect::Dialect::Claude);

        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();
        assert!(
            sync_diff2
                .items
                .iter()
                .any(|entry| matches!(entry, diff::DiffEntry::Update { .. })),
            "expected Update after dialect change, got {:?}",
            sync_diff2.items
        );
    }

    #[test]
    fn validate_skill_refs_ignores_stale_installed_agent_content() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("design-lead.md", "# Design Lead\n")], &[]);
        fs::create_dir_all(fixture.managed_root().join("agents")).unwrap();
        fs::write(
            fixture.managed_root().join("agents/design-lead.md"),
            "---\nskills: [handoff]\n---\n# Stale Design Lead\n",
        )
        .unwrap();

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();

        let warnings = validate_skill_refs(&target);

        assert!(
            warnings.is_empty(),
            "target source removed the missing ref, but stale installed content produced {warnings:?}"
        );
    }

    #[test]
    fn validate_skill_refs_warns_for_missing_target_source_ref() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(
            &[("coder.md", "---\nskills: [missing-skill]\n---\n# Coder\n")],
            &[],
        );

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();

        let warnings = validate_skill_refs(&target);

        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            ValidationWarning::MissingSkill {
                agent,
                skill_name,
                suggestion,
            } => {
                assert_eq!(agent.name, "coder");
                assert_eq!(skill_name, "missing-skill");
                assert_eq!(suggestion, &None);
            }
        }
    }

    #[test]
    fn validate_skill_refs_uses_rewritten_content() {
        let fixture = TestFixture::new();
        let source_path = fixture.project_root().join("source-agent.md");
        fs::write(
            &source_path,
            "---\nskills: [old-skill]\n---\n# Source content before rewrite\n",
        )
        .unwrap();
        let skill_path = fixture.project_root().join("skills").join("new-skill");
        fs::create_dir_all(&skill_path).unwrap();
        fs::write(skill_path.join("SKILL.md"), "# New Skill\n").unwrap();

        let source_name = SourceName::from("base");
        let source_id = SourceId::Path {
            canonical: fixture.project_root().to_path_buf(),
            subpath: None,
        };
        let mut items = IndexMap::new();
        items.insert(
            DestPath::new("agents/coder.md").unwrap(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Agent,
                    name: "coder".into(),
                },
                source_name: source_name.clone(),
                origin: SourceOrigin::Dependency(source_name.clone()),
                source_id: source_id.clone(),
                source_path,
                dest_path: DestPath::new("agents/coder.md").unwrap(),
                source_hash: ContentHash::from("sha256:source"),
                is_flat_skill: false,
                rewritten_content: Some(
                    "---\nskills: [new-skill]\n---\n# Rewritten content\n".to_string(),
                ),
            },
        );
        items.insert(
            DestPath::new("skills/new-skill").unwrap(),
            TargetItem {
                id: ItemId {
                    kind: ItemKind::Skill,
                    name: "new-skill".into(),
                },
                source_name: source_name.clone(),
                origin: SourceOrigin::Dependency(source_name),
                source_id,
                source_path: skill_path,
                dest_path: DestPath::new("skills/new-skill").unwrap(),
                source_hash: ContentHash::from("sha256:skill"),
                is_flat_skill: false,
                rewritten_content: None,
            },
        );
        let target = TargetState { items };

        let warnings = validate_skill_refs(&target);

        assert!(
            warnings.is_empty(),
            "validation should use rewritten content instead of stale source content: {warnings:?}"
        );
    }

    #[test]
    fn source_update_detects_changes() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("coder.md", "# Version 1")], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // First sync
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        // Update source content
        let agents_dir = fixture.tree_path(src_idx).join("agents");
        fs::write(agents_dir.join("coder.md"), "# Version 2").unwrap();

        // Rebuild target with updated content
        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

        // Should detect an Update
        assert_eq!(sync_diff2.items.len(), 1);
        assert!(matches!(
            &sync_diff2.items[0],
            diff::DiffEntry::Update { .. }
        ));
    }

    #[test]
    fn local_modification_preserved() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("coder.md", "# Original")], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // First sync
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        // Locally modify the installed file
        fs::write(
            fixture.managed_root().join("agents/coder.md"),
            "# Locally modified",
        )
        .unwrap();

        // Re-sync (source unchanged)
        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

        // Should detect LocalModified
        assert_eq!(sync_diff2.items.len(), 1);
        assert!(matches!(
            &sync_diff2.items[0],
            diff::DiffEntry::LocalModified { .. }
        ));

        // Plan should KeepLocal
        let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
        assert!(matches!(
            &sync_plan2.actions[0],
            plan::PlannedAction::KeepLocal { .. }
        ));
    }

    #[test]
    fn force_overwrites_local_modifications() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("coder.md", "# Original")], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // First sync
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        // Locally modify the installed file
        fs::write(
            fixture.managed_root().join("agents/coder.md"),
            "# Locally modified",
        )
        .unwrap();

        // Update source too (triggers conflict)
        let agents_dir = fixture.tree_path(src_idx).join("agents");
        fs::write(agents_dir.join("coder.md"), "# Upstream update").unwrap();

        // Re-sync with --force
        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

        let force_options = SyncOptions {
            force: true,
            ..SyncOptions::default()
        };
        let sync_plan2 = create_sync_plan(&sync_diff2, &force_options, &cache_dir);
        assert!(matches!(
            &sync_plan2.actions[0],
            plan::PlannedAction::Overwrite { .. }
        ));

        let result2 = apply::execute(
            fixture.managed_root(),
            &sync_plan2,
            &force_options,
            &cache_dir,
        )
        .unwrap();
        assert!(matches!(
            result2.outcomes[0].action,
            apply::ActionTaken::Updated
        ));

        // File should have upstream content
        let content = fs::read_to_string(fixture.managed_root().join("agents/coder.md")).unwrap();
        assert_eq!(content, "# Upstream update");
    }

    #[test]
    fn orphan_removed_when_source_drops_item() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(
            &[("coder.md", "# Coder"), ("reviewer.md", "# Reviewer")],
            &[],
        );

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // First sync — install both
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let first_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

        assert!(fixture.managed_root().join("agents/coder.md").exists());
        assert!(fixture.managed_root().join("agents/reviewer.md").exists());

        // Remove reviewer from source
        fs::remove_file(fixture.tree_path(src_idx).join("agents/reviewer.md")).unwrap();

        // Re-sync
        let (target2, _) = target::build_with_collisions(&graph, &config).unwrap();
        let sync_diff2 =
            diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

        // Should have one Unchanged and one Orphan
        let orphan_count = sync_diff2
            .items
            .iter()
            .filter(|e| matches!(e, diff::DiffEntry::Orphan { .. }))
            .count();
        assert_eq!(orphan_count, 1);

        let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
        let result2 =
            apply::execute(fixture.managed_root(), &sync_plan2, &options, &cache_dir).unwrap();

        // Reviewer should be removed
        assert!(!fixture.managed_root().join("agents/reviewer.md").exists());
        // Coder should still be there
        assert!(fixture.managed_root().join("agents/coder.md").exists());

        // Check remove outcome
        let removed = result2
            .outcomes
            .iter()
            .any(|o| matches!(o.action, apply::ActionTaken::Removed));
        assert!(removed);
    }

    #[test]
    fn dry_run_produces_plan_without_changes() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("coder.md", "# Coder")], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();

        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let dry_options = SyncOptions {
            dry_run: true,
            ..SyncOptions::default()
        };

        let sync_plan = create_sync_plan(&sync_diff, &dry_options, &cache_dir);
        assert!(!sync_plan.actions.is_empty());

        // Execute in dry-run mode
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &dry_options, &cache_dir).unwrap();
        assert!(!result.outcomes.is_empty());

        // No files should have been created
        assert!(!fixture.managed_root().join("agents/coder.md").exists());
    }

    #[test]
    fn lock_written_after_apply() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[("coder.md", "# Coder")], &[]);

        let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

        // Full pipeline minus actual sync() (which needs real config files)
        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();

        let new_lock =
            crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();
        crate::lock::write(fixture.project_root(), &new_lock).unwrap();

        // Verify lock file exists and is valid
        let reloaded = crate::lock::load(fixture.project_root()).unwrap();
        assert_eq!(reloaded.items.len(), 1);
        assert!(reloaded.items.contains_key("agent/coder"));

        let item = &reloaded.items["agent/coder"];
        assert_eq!(item.kind, ItemKind::Agent);
        assert!(!item.source_checksum.is_empty());
        assert!(!item.outputs[0].installed_checksum.is_empty());
    }

    #[test]
    fn two_sources_no_collision() {
        let mut fixture = TestFixture::new();
        let src_a = fixture.add_source(&[("coder.md", "# Coder from A")], &[]);
        let src_b = fixture.add_source(&[("reviewer.md", "# Reviewer from B")], &[]);

        let (graph, config) = make_graph_config(
            &fixture,
            vec![
                ("source-a", src_a, FilterMode::All),
                ("source-b", src_b, FilterMode::All),
            ],
        );

        let (target, renames) = target::build_with_collisions(&graph, &config).unwrap();
        assert!(renames.is_empty());
        assert_eq!(target.items.len(), 2);

        let lock = LockFile::empty();
        let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
        let cache_dir = fixture.project_root().join(".mars/cache/bases");
        let options = SyncOptions::default();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();

        assert!(fixture.managed_root().join("agents/coder.md").exists());
        assert!(fixture.managed_root().join("agents/reviewer.md").exists());
        assert_eq!(result.outcomes.len(), 2);
    }

    // === Tests for OnlySkills / OnlyAgents filter in pipeline ===

    #[test]
    fn pipeline_only_skills_filter() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(
            &[("coder.md", "# Coder agent")],
            &[("planning", "# Planning skill")],
        );

        let (graph, config) =
            make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlySkills)]);

        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        // Should only have the skill, not the agent
        assert_eq!(target.items.len(), 1);
        assert!(target.items.contains_key("skills/planning"));
    }

    #[test]
    fn pipeline_only_agents_filter() {
        let mut fixture = TestFixture::new();
        // Agent with a skill dependency in frontmatter
        let agent_content = "---\nskills:\n  - planning\n---\n# Coder agent";
        let src_idx = fixture.add_source(
            &[("coder.md", agent_content)],
            &[
                ("planning", "# Planning skill"),
                ("standalone", "# Standalone skill"),
            ],
        );

        let (graph, config) =
            make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlyAgents)]);

        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        // Should have the agent + its transitive skill dep, but NOT standalone
        assert_eq!(target.items.len(), 2);
        assert!(target.items.contains_key("agents/coder.md"));
        assert!(target.items.contains_key("skills/planning"));
        assert!(!target.items.contains_key("skills/standalone"));
    }

    #[test]
    fn pipeline_only_agents_no_agents_source() {
        let mut fixture = TestFixture::new();
        let src_idx = fixture.add_source(&[], &[("planning", "# Planning skill")]);

        let (graph, config) =
            make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlyAgents)]);

        let (target, _) = target::build_with_collisions(&graph, &config).unwrap();
        // No agents means nothing gets installed
        assert_eq!(target.items.len(), 0);
    }
}
