use mars_schema::item::ItemKind;
pub mod apply;
pub mod diff;
pub mod filter;
mod local_items;
pub mod mutation;
pub(crate) mod ownership;
pub(crate) mod package_dir;
pub mod plan;
pub(crate) mod recovery;
pub mod rewrite;
pub mod target;
pub mod types;
mod upgrades;
pub(crate) mod validate;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;

use crate::config::{Config, EffectiveConfig, LocalConfig, Settings};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, LossinessMode};
use crate::error::MarsError;
use crate::fs::FileLock;
use crate::lock::LockFile;
use crate::resolve::{ResolveOptions, ResolvedGraph};
use crate::source::GlobalCache;
use crate::sync::apply::ApplyResult;
pub use crate::sync::apply::SyncOptions;
use crate::sync::target::TargetState;
use crate::types::managed_cmd;
use crate::types::{MarsContext, SourceName};
use crate::validate::ValidationWarning;

pub use crate::sync::mutation::{ConfigMutation, DependencyUpsertChange};

/// Report from a completed sync operation.
#[derive(Debug)]
pub struct SyncReport {
    pub applied: ApplyResult,
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
    /// Present when a recovery command persisted intent but stopped before
    /// materialization because at least one hook surface was unreadable.
    pub recovery_halt: Option<RecoveryHalt>,
    /// Version fallbacks caused by package engine requirements.
    pub engine_fallbacks: Vec<crate::resolve::EngineFallback>,
}

/// A source package preventing a recovery command from entering the compiler.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryBlocker {
    pub package: String,
    pub version: String,
    pub hook_names: Vec<String>,
    pub guidance: String,
    pub suggested_command: String,
}

/// Successful intent persistence that still requires recovery work.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryHalt {
    pub persisted: Vec<String>,
    pub blockers: Vec<RecoveryBlocker>,
    pub next_step: String,
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
    /// Whether a recovery command may persist intent and halt when resolution
    /// finds a hook surface the compiler cannot read.
    pub recovery: RecoveryPolicy,
    /// Whether lossiness warnings are included in the returned report.
    /// `Surface` for `mars sync` / `mars upgrade`; `Hidden` for validate/export/add/repair.
    pub lossiness_mode: LossinessMode,
}

/// Schema handling policy for content encountered during sync resolution.
///
/// Strict is the safe default: only commands whose purpose is to recover a
/// locked-out graph may opt into deferring materialization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecoveryPolicy {
    #[default]
    Strict,
    DeferOnUnreadable,
    /// Defer on unreadable sources and rebuild a corrupt lock in memory.
    ///
    /// The corrupt file remains untouched unless the full pipeline reaches
    /// finalization and replaces it with a rebuilt lock.
    Repair,
}

impl RecoveryPolicy {
    fn defers_on_unreadable(self) -> bool {
        matches!(self, Self::DeferOnUnreadable | Self::Repair)
    }
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
    /// What `mars provide` installed here, read under the sync lock. Never written by sync.
    pub provided: crate::provided::ProvidedNames,
    pub dependency_changes: Vec<DependencyUpsertChange>,
    /// Intentional keepalive — holds the sync file lock for the duration of the pipeline. Dropping this field releases the lock. A dry run holds it only when `.mars/sync.lock` already exists, and never creates it: it mutates nothing the lock guards.
    #[allow(dead_code)]
    pub sync_lock: Option<FileLock>,
}

impl LoadedConfig {
    /// Whether a canonical destination is Mars-owned: claimed by `mars.lock` or provided.
    pub(crate) fn ownership(&self) -> ownership::Ownership<'_> {
        ownership::Ownership::new(&self.old_lock, &self.provided)
    }
}

/// Phase 2: Resolved dependency graph.
pub(crate) struct ResolvedState {
    pub loaded: LoadedConfig,
    pub graph: ResolvedGraph,
    pub upgrades_available: usize,
    /// Owns every normalized package tree until the compiler has passed its
    /// complete diagnostic gate. Durable staging is published only afterward.
    pub preparation: crate::platform::fs::PendingStagingGeneration,
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
    pub config_entry_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub removed_config_entry_outputs: Vec<(String, String)>,
    pub compiled_native_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub removed_native_outputs: Vec<crate::compiler::RemovedNativeOutput>,
}

/// Execute the unified sync pipeline.
///
/// Orchestrates phase functions, each consuming the prior phase's output struct.
pub fn execute(ctx: &MarsContext, request: &SyncRequest) -> Result<SyncReport, MarsError> {
    validate_request(request)?;
    let mut diag = DiagnosticCollector::with_lossiness_mode(request.lossiness_mode);
    let ir = crate::reader::read(ctx, request, &mut diag, true)?;
    let unreadable_hook_surfaces = &ir.resolved.graph.unreadable_hook_surfaces;
    if request.recovery.defers_on_unreadable() && !unreadable_hook_surfaces.is_empty() {
        persist_pending_config_mutation(ctx, &ir.resolved.loaded, request)?;
        let recovery_halt = build_recovery_halt(&ir.resolved, unreadable_hook_surfaces, request);
        return Ok(SyncReport {
            applied: ApplyResult {
                outcomes: Vec::new(),
            },
            diagnostics: diag.drain(),
            dependency_changes: ir.resolved.loaded.dependency_changes,
            upgrades_available: ir.resolved.upgrades_available,
            target_outcomes: Vec::new(),
            dry_run: request.options.dry_run,
            native_emitted: Vec::new(),
            native_removed: Vec::new(),
            recovery_halt: Some(recovery_halt),
            engine_fallbacks: diag.take_engine_fallbacks(),
        });
    }
    crate::compiler::compile(ctx, ir, request, &mut diag)
}

/// Run the same preparation/compiler path while returning schema errors as
/// report data. Validation is dry-run only, so it never authorizes mutations.
pub(crate) fn execute_validation(
    ctx: &MarsContext,
    request: &SyncRequest,
) -> Result<SyncReport, MarsError> {
    validate_request(request)?;
    debug_assert!(request.options.dry_run);
    let mut diag = DiagnosticCollector::with_lossiness_mode(request.lossiness_mode);
    let ir = crate::reader::read(ctx, request, &mut diag, false)?;
    crate::compiler::compile_reporting(ctx, ir, request, &mut diag)
}

fn build_recovery_halt(
    resolved: &ResolvedState,
    unreadable_hook_surfaces: &BTreeMap<SourceName, std::collections::BTreeSet<String>>,
    request: &SyncRequest,
) -> RecoveryHalt {
    let persisted = persisted_intent_descriptions(resolved, request);
    let blockers = unreadable_hook_surfaces
        .iter()
        .map(|(source_name, hook_names)| {
            let node = resolved
                .graph
                .nodes
                .get(source_name)
                .expect("unreadable surface belongs to a resolved node");
            let version = node
                .resolved_ref
                .version
                .as_ref()
                .map(ToString::to_string)
                .or_else(|| node.resolved_ref.version_tag.clone())
                .or_else(|| {
                    node.manifest
                        .as_ref()
                        .map(|manifest| manifest.package.version.clone())
                })
                .or_else(|| node.resolved_ref.commit.as_ref().map(ToString::to_string))
                .unwrap_or_else(|| "path".to_string());
            let parents: Vec<_> = resolved
                .graph
                .nodes
                .values()
                .filter(|candidate| candidate.deps.contains(source_name))
                .map(|candidate| candidate.source_name.to_string())
                .collect();
            let upgrade_cmd =
                managed_cmd(&format!("mars upgrade {source_name}")).into_owned();
            let (guidance, suggested_command) = match &request.mutation {
                Some(ConfigMutation::RemoveDependency { name })
                    if name == source_name && !parents.is_empty() =>
                {
                    (
                        format!(
                            "removed direct dependency `{name}`, but `{source_name}` is still required by {} and remains legacy; override it",
                            parents.join(", ")
                        ),
                        managed_cmd(&format!("mars override {source_name} --path <path>")).into_owned(),
                    )
                }
                None if matches!(request.resolution, ResolutionMode::Maximize { .. }) => {
                    let bump = matches!(
                        request.resolution,
                        ResolutionMode::Maximize { bump: true, .. }
                    );
                    if bump {
                        (
                            format!(
                                "newest available `{source_name}@{version}` still uses the removed hook schema; override or remove it"
                            ),
                            format!(
                                "{cmd_override} --path <path> or {cmd_remove}",
                                cmd_override = managed_cmd(&format!("mars override {source_name}")),
                                cmd_remove = managed_cmd(&format!("mars remove {source_name}")),
                            ),
                        )
                    } else {
                        (
                            format!(
                                "newest compatible `{source_name}@{version}` still uses the removed hook schema; try --bump to escape the version constraint, or override/remove"
                            ),
                            managed_cmd(&format!("mars upgrade {source_name} --bump")).into_owned(),
                        )
                    }
                }
                None if request.options.force => (
                    format!(
                        "cannot repair while `{source_name}@{version}` uses the removed hook schema; upgrade, override, or remove it"
                    ),
                    upgrade_cmd.clone(),
                ),
                _ => (
                    format!(
                        "`{source_name}@{version}` still uses the removed hook schema; upgrade, override, or remove it"
                    ),
                    upgrade_cmd,
                ),
            };
            RecoveryBlocker {
                package: source_name.to_string(),
                version,
                hook_names: hook_names.iter().cloned().collect(),
                guidance,
                suggested_command,
            }
        })
        .collect();
    RecoveryHalt {
        persisted,
        blockers,
        next_step: format!("then run `{}`", managed_cmd("mars sync")),
    }
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

    // A dry run is a preview: it must leave the project's files as it found
    // them. It never creates the lock file or reclaims a pending generation,
    // and stages in scratch space outside the project (`resolve_graph`). It
    // still takes the flock when a real sync already created it: recovery reads
    // `mars.lock` and the intent journal, and a publisher running between those
    // reads would show it a torn snapshot.
    let sync_lock = if request.options.dry_run {
        crate::fs::FileLock::acquire_existing(&mars_dir.join("sync.lock"))?
    } else {
        // The lock is the sole project-state write allowed before preparation's
        // complete diagnostic gate. All package trees live in owned scratch space.
        let lock = crate::fs::FileLock::acquire(&mars_dir.join("sync.lock"))?;
        if crate::platform::fs::recover_pending_staging(&mars_dir)? {
            diag.warn(
                "sync-recovered-staging",
                "reclaimed an interrupted canonical staging generation",
            );
        }
        if crate::platform::fs::recover_interrupted_skill_installs(&mars_dir)? {
            diag.warn(
                "sync-recovered-skill-install",
                "reclaimed an interrupted skill install left staged under .mars/skills",
            );
        }
        Some(lock)
    };

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

    if request.options.ignore_requires_mars {
        diag.warn(
            "requires-mars-disabled",
            "`requires-mars` compatibility checks are disabled by --ignore-requires-mars",
        );
    }
    if request.options.ignore_requires_meridian {
        diag.warn(
            "requires-meridian-disabled",
            "`requires-meridian` compatibility checks are disabled by --ignore-requires-meridian",
        );
    }
    if let Some(package) = config.package.as_ref() {
        let options = to_resolve_options(&request.resolution, &request.options);
        crate::resolve::check_consumer_package_requirements(package, &options)?;
    }

    // Load existing lock file, routing load diagnostics through sync diagnostics.
    let (mut old_lock, lock_diagnostics) = match crate::lock::load_with_diagnostics(project_root) {
        Ok(loaded) => loaded,
        Err(MarsError::Lock(crate::error::LockError::Corrupt { message }))
            if request.recovery == RecoveryPolicy::Repair =>
        {
            diag.warn(
                "corrupt-lock-rebuild",
                format!("{message}; lock is corrupt, rebuilding from mars.toml + dependencies"),
            );
            (LockFile::empty(), Vec::new())
        }
        Err(err) => return Err(err),
    };
    diag.extend(lock_diagnostics);
    let recovered = recovery::recover(project_root, &mut old_lock)?;
    if recovered > 0 && request.options.frozen {
        return Err(MarsError::FrozenViolation {
            message: "interrupted canonical writes require ownership recovery; run sync without --frozen first".into(),
        });
    }
    if recovered > 0 {
        diag.warn(
            "sync-recovered",
            format!(
                "recovered ownership of {recovered} canonical outputs from an interrupted sync"
            ),
        );
    }

    let provided = crate::provided::ProvidedNames::load(project_root, diag)?;

    Ok(LoadedConfig {
        config,
        local,
        effective,
        old_lock,
        provided,
        dependency_changes,
        sync_lock,
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
    let source_provider = crate::resolve::RealSourceProvider::new(&cache, &ctx.project_root);
    let source_overrides = loaded
        .local
        .overrides
        .iter()
        .map(|(name, entry)| {
            let path = if entry.path.is_absolute() {
                entry.path.clone()
            } else {
                ctx.project_root.join(&entry.path)
            };
            (name.clone(), path)
        })
        .collect();
    let preparation = if request.options.dry_run {
        crate::platform::fs::PendingStagingGeneration::scratch()?
    } else {
        crate::platform::fs::PendingStagingGeneration::create(&ctx.project_root.join(".mars"))?
    };
    let resolve_options = to_resolve_options(&request.resolution, &request.options)
        .with_source_overrides(source_overrides);
    let graph = crate::resolve::resolve(
        &loaded.effective,
        &source_provider,
        Some(&loaded.old_lock),
        &resolve_options,
        diag,
    )?;
    if let Some(ConfigMutation::SetOverride { source_name, .. }) = &request.mutation
        && !graph.nodes.contains_key(source_name)
    {
        return Err(MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("dependency `{source_name}` not found in the resolved project graph"),
        });
    }
    for override_name in loaded.local.overrides.keys() {
        if !graph.nodes.contains_key(override_name) {
            diag.warn(
                "override-missing-dep",
                format!(
                    "override `{override_name}` references a dependency not in the resolved project graph"
                ),
            );
        }
    }
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
        preparation,
    })
}

/// Shared result of graph collection, rename application, and validation.
pub(crate) struct PreparedTarget {
    pub target: target::TargetState,
    pub rename_index: rewrite::RenameIndex,
    pub explicit_renames: Vec<target::ExplicitRename>,
    pub collision_renames: Vec<target::CollisionRename>,
    pub dep_precedence: Vec<SourceName>,
}

/// One resolved target after selection, local extension, MCP collision
/// resolution and MCP directory materialization, before renames apply.
///
/// A caller that prunes items (sync drops paths holding unmanaged content) must
/// do so before the rename pass, because renames rewrite references to the items
/// that survive.
pub(crate) struct BuiltTarget {
    pub target: target::TargetState,
    pub explicit_renames: Vec<target::ExplicitRename>,
    pub collision_renames: Vec<target::CollisionRename>,
}

/// Build one resolved graph target and settle everything that decides which
/// items survive: graph collection, the caller's local extension, MCP name
/// collisions, and each winning server's installed directory.
///
/// Sync extends the dependency target with project-local items here; publish
/// builds use the graph target unchanged. Keeping the extension inside this
/// function preserves the own-package shadowing rules.
///
/// `local_source` names the consumer's own package: it wins MCP name collisions
/// silently. Sync's local package is `_self`; a plugin build's is its root package.
pub(crate) fn build_target_state(
    graph: &crate::resolve::ResolvedGraph,
    effective: &EffectiveConfig,
    local_source: &SourceName,
    diag: &mut DiagnosticCollector,
    extend: impl FnOnce(&mut target::TargetState, &mut DiagnosticCollector) -> Result<(), MarsError>,
) -> Result<BuiltTarget, MarsError> {
    let (mut target, explicit_renames, collision_renames) =
        target::build_with_collisions_and_diag(graph, effective, diag)?;
    extend(&mut target, diag)?;
    target.resolve_mcp_collisions(
        &effective.settings.managed_targets(),
        local_source.as_str(),
        diag,
    )?;
    Ok(BuiltTarget {
        target,
        explicit_renames,
        collision_renames,
    })
}

/// Build the rename view of the surviving items and rewrite every reference to
/// them.
///
/// Skill refs are checked by each caller after this returns, with its own context: sync knows
/// what this machine installed and what the project's own package declares, a plugin build has
/// neither.
pub(crate) fn apply_target_renames(
    built: BuiltTarget,
    graph: &crate::resolve::ResolvedGraph,
    effective: &EffectiveConfig,
    diag: &mut DiagnosticCollector,
) -> Result<PreparedTarget, MarsError> {
    let BuiltTarget {
        mut target,
        explicit_renames,
        collision_renames,
    } = built;
    let rename_index = rewrite::RenameIndex::new(&explicit_renames, &collision_renames, &target);
    let dep_precedence: Vec<SourceName> = effective.dependencies.keys().cloned().collect();
    if !rename_index.is_empty() {
        for warning in rewrite::apply_renames(&mut target, &rename_index, graph, &dep_precedence)? {
            diag.warn("rewrite-warning", warning);
        }
    }

    Ok(PreparedTarget {
        target,
        rename_index,
        explicit_renames,
        collision_renames,
        dep_precedence,
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
    let local_source = crate::types::SourceOrigin::LocalPackage.to_string().into();

    let mut built = build_target_state(
        &resolved.graph,
        &resolved.loaded.effective,
        &local_source,
        diag,
        |target_state, diag| {
            local_items::merge_local_items(ctx, &resolved, local_items, request, target_state, diag)
        },
    )?;

    // Prevent managed installs from overwriting unmanaged files. This runs over
    // every item, hook and MCP directories included, and before renames rewrite
    // references, so a pruned item is neither referenced nor installed.
    for collision in target::check_unmanaged_collisions(
        managed_root,
        &resolved.loaded.ownership(),
        &built.target,
        request.options.force,
    ) {
        diag.warn(
            "unmanaged-collision",
            format!(
                "source `{}` collides with unmanaged path `{}` — leaving existing content untouched",
                collision.source_name, collision.path
            ),
        );
        built.target.items.shift_remove(&collision.path);
    }

    let PreparedTarget {
        target: target_state,
        explicit_renames: renames,
        collision_renames,
        ..
    } = apply_target_renames(built, &resolved.graph, &resolved.loaded.effective, diag)?;

    let warnings = validate::validate_skill_refs(
        &target_state,
        &resolved.graph,
        Some(&resolved.loaded.provided),
        resolved
            .loaded
            .config
            .package
            .as_ref()
            .map(|package| package.uses_providers.as_slice()),
    );

    validate::warn_config_dangles_after_rename(
        &renames,
        &collision_renames,
        &target_state,
        &resolved.loaded,
        diag,
    );

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
    let sync_plan = plan::create(&sync_diff, &request.options, diag);

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

    recovery::validate_plan(&planned.plan)?;

    // Persist config/local only after validation gate and before apply.
    persist_pending_config_mutation(ctx, &planned.targeted.resolved.loaded, request)?;

    // Apply plan to .mars/ canonical store (D25).
    // Content is written to .mars/agents/ and .mars/skills/, then
    // sync_targets() copies to all managed target directories.
    if !request.options.dry_run {
        recovery::prepare(
            project_root,
            &planned.plan,
            &planned.targeted.resolved.graph,
            &planned.targeted.resolved.loaded.old_lock,
        )?;
    }
    let applied = apply::execute(&mars_dir, &planned.plan, &request.options)?;

    Ok(AppliedState { planned, applied })
}

fn has_bump_version_changes(loaded: &LoadedConfig, request: &SyncRequest) -> bool {
    has_version_changes(&loaded.dependency_changes)
        && matches!(
            request.resolution,
            ResolutionMode::Maximize { bump: true, .. }
        )
}

/// Persist only the user's pending intent mutation. This is shared by the
/// normal apply phase and the recovery halt at the reader/compiler boundary.
fn persist_pending_config_mutation(
    ctx: &MarsContext,
    loaded: &LoadedConfig,
    request: &SyncRequest,
) -> Result<(), MarsError> {
    if request.options.dry_run {
        return Ok(());
    }
    let bump_changed = has_bump_version_changes(loaded, request);
    match &request.mutation {
        Some(ConfigMutation::SetOverride { .. }) => {
            crate::config::save_local(&ctx.project_root, &loaded.local)?;
        }
        Some(
            ConfigMutation::UpsertDependency { .. }
            | ConfigMutation::BatchUpsert(..)
            | ConfigMutation::RemoveDependency { .. }
            | ConfigMutation::SetRename { .. },
        ) => {
            crate::config::save(&ctx.project_root, &loaded.config)?;
        }
        None if bump_changed => {
            crate::config::save(&ctx.project_root, &loaded.config)?;
        }
        None => {}
    }
    Ok(())
}

fn persisted_intent_descriptions(resolved: &ResolvedState, request: &SyncRequest) -> Vec<String> {
    let dry_prefix = if request.options.dry_run {
        "would persist"
    } else {
        "persisted"
    };
    match &request.mutation {
        Some(ConfigMutation::SetOverride {
            source_name,
            local_path,
        }) => vec![format!(
            "{dry_prefix} override for `{source_name}` to `{}` in mars.local.toml",
            local_path.display()
        )],
        Some(ConfigMutation::RemoveDependency { name }) => {
            vec![format!(
                "{dry_prefix} removal of direct dependency `{name}` in mars.toml"
            )]
        }
        Some(_) => vec![format!("{dry_prefix} config mutation")],
        None if has_bump_version_changes(&resolved.loaded, request) => resolved
            .loaded
            .dependency_changes
            .iter()
            .filter(|change| change.old_version != change.new_version)
            .map(|change| {
                format!(
                    "{dry_prefix} bumped constraint for `{}` to `{}` in mars.toml",
                    change.name,
                    change.new_version.as_deref().unwrap_or("latest")
                )
            })
            .collect(),
        None => vec!["nothing persisted".to_string()],
    }
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
            config_entry_outputs: Vec::new(),
            removed_config_entry_outputs: Vec::new(),
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
    let target_outcomes_source = match &agent_surface_policy {
        crate::compiler::AgentSurfacePolicy::SuppressAll => {
            filtered_outcomes = crate::compiler::suppress_agent_outcomes(&applied.applied.outcomes);
            &filtered_outcomes
        }
        crate::compiler::AgentSurfacePolicy::EmitSelective(_) => {
            filtered_outcomes = crate::compiler::omit_agent_outcomes(&applied.applied.outcomes);
            &filtered_outcomes
        }
        crate::compiler::AgentSurfacePolicy::EmitAll => &applied.applied.outcomes,
    };
    let mut orphan_preserve_paths =
        crate::compiler::config_entries::file_hook_output_preserve_paths(old_lock);
    for (target, paths) in crate::compiler::native_agent_orphan_preserve_paths(old_lock, &targets) {
        orphan_preserve_paths
            .entry(target)
            .or_default()
            .extend(paths);
    }
    let orphan_preserve = (!orphan_preserve_paths.is_empty()).then_some(&orphan_preserve_paths);

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
        config_entry_outputs: Vec::new(),
        removed_config_entry_outputs: Vec::new(),
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
        let mut confirmed_output_removals: Vec<(String, String)> = state
            .target_outcomes
            .iter()
            .flat_map(|outcome| {
                outcome
                    .removed_dest_paths
                    .iter()
                    .map(|dest_path| (outcome.target.clone(), dest_path.clone()))
            })
            .collect();
        confirmed_output_removals.extend(state.removed_config_entry_outputs.iter().cloned());
        confirmed_output_removals.extend(state.removed_native_outputs.iter().cloned());
        crate::lock::apply_target_sync_outputs(&mut new_lock, &state.target_outcomes);
        crate::lock::apply_removed_native_outputs(
            &mut new_lock,
            &state.removed_config_entry_outputs,
        );
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.config_entry_outputs)?;
        crate::lock::apply_removed_native_outputs(&mut new_lock, &state.removed_native_outputs);
        crate::lock::apply_compiled_native_outputs(&mut new_lock, &state.compiled_native_outputs)?;
        confirmed_output_removals.extend(retry_tombstone_removals(
            project_root,
            old_lock,
            &new_lock,
            diag,
        ));
        crate::lock::retain_unremoved_noncanonical_outputs(
            &mut new_lock,
            old_lock,
            &confirmed_output_removals,
        );
        if let Some(warning) =
            crate::compiler::persist_lock_then_native_agent_manifest(project_root, &new_lock)?
        {
            diag.warn("native-agent-manifest-write", warning);
        }
        recovery::complete(project_root)?;

        // Best-effort models cache refresh: ensure the catalog covers any
        // new aliases we're about to persist. Sync never aborts on refresh
        // failure — warn and continue.
        let mars_path = ctx.project_root.join(".mars");
        let settings = &state
            .applied
            .planned
            .targeted
            .resolved
            .loaded
            .effective
            .settings;
        let ttl = settings.models_cache_ttl_hours;
        let providers = settings
            .catalog_providers
            .clone()
            .unwrap_or_else(crate::models::default_catalog_providers);
        let refresh = crate::models::resolve_models_refresh_control(
            request.options.refresh_models,
            request.options.no_refresh_models,
        )?;
        match crate::models::ensure_fresh_with_catalog_providers(
            &mars_path,
            ttl,
            refresh.catalog_mode,
            &providers,
        ) {
            Ok((_, outcome)) => {
                if let Some(warning) = crate::models::refresh_warning(&outcome) {
                    diag.warn("models-cache-refresh", warning);
                }
            }
            Err(err) => {
                diag.warn(
                    "models-cache-refresh",
                    format!("failed to refresh models cache: {err}"),
                );
            }
        }
    }

    for warning in &state.applied.planned.targeted.warnings {
        diag.warn(warning.code(), warning.message());
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
        diagnostics,
        dependency_changes,
        upgrades_available,
        target_outcomes: state.target_outcomes,
        dry_run: request.options.dry_run,
        native_emitted,
        native_removed,
        recovery_halt: None,
        engine_fallbacks: diag.take_engine_fallbacks(),
    })
}

fn retry_tombstone_removals(
    project_root: &Path,
    old_lock: &crate::lock::LockFile,
    current_lock: &crate::lock::LockFile,
    diag: &mut DiagnosticCollector,
) -> Vec<(String, String)> {
    let mut removed = Vec::new();
    for item in old_lock.items.values().filter(|item| {
        !item
            .outputs
            .iter()
            .any(|output| output.target_root == crate::lock::CANONICAL_TARGET_ROOT)
    }) {
        for output in &item.outputs {
            let current_pass_owns_output = current_lock.items.values().any(|current_item| {
                current_item.outputs.iter().any(|current_output| {
                    current_output.target_root == crate::lock::CANONICAL_TARGET_ROOT
                }) && current_item.outputs.iter().any(|current_output| {
                    current_output.target_root == output.target_root
                        && crate::target::dest_paths_equivalent(
                            current_output.dest_path.as_str(),
                            output.dest_path.as_str(),
                        )
                })
            });
            if output.target_root == crate::lock::CANONICAL_TARGET_ROOT || current_pass_owns_output
            {
                continue;
            }

            let path = project_root
                .join(&output.target_root)
                .join(output.dest_path.as_str());
            let result = if matches!(item.kind, ItemKind::Agent)
                || (matches!(item.kind, ItemKind::Hook)
                    && !output.dest_path.as_str().starts_with("hooks/"))
            {
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                }
            } else {
                crate::platform::fs::safe_remove(&path)
            };

            match result {
                Ok(()) => removed.push((
                    output.target_root.clone(),
                    output.dest_path.as_str().to_string(),
                )),
                Err(error) => diag.warn(
                    "tombstone-remove",
                    format!(
                        "could not remove tombstoned output `{}`: {error}",
                        path.display()
                    ),
                ),
            }
        }
    }
    removed
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
                return Err(mutation::dependency_not_found(
                    effective.dependencies.keys(),
                    name,
                ));
            }
        }
    }

    Ok(())
}

fn to_resolve_options(mode: &ResolutionMode, options: &SyncOptions) -> ResolveOptions {
    let mut resolve_options = if options.frozen {
        ResolveOptions::frozen()
    } else {
        match mode {
            ResolutionMode::Normal => ResolveOptions::sync(),
            ResolutionMode::Maximize { targets, bump } => {
                ResolveOptions::upgrade(targets.clone(), *bump)
            }
        }
    };
    resolve_options.ignore_requires_mars = options.ignore_requires_mars;
    resolve_options.ignore_requires_meridian = options.ignore_requires_meridian;
    resolve_options
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
