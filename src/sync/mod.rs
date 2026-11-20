pub mod apply;
pub mod diff;
mod finalize;
mod halt;
mod kept_hooks;
mod local_items;
pub mod mutation;
pub(crate) mod ownership;
pub(crate) mod package_dir;
mod package_tools;
pub mod plan;
pub(crate) mod recovery;
pub mod rewrite;
pub mod target;
pub mod types;
pub(crate) mod upgrades;
pub(crate) mod validate;

use std::collections::BTreeMap;
use std::collections::HashSet;

use crate::config::{Config, EffectiveConfig, LocalConfig, Settings};
use crate::diagnostic::{Diagnostic, DiagnosticCollector, ItemGroup, LossinessMode};
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

pub(crate) use crate::sync::finalize::finalize;
pub use crate::sync::halt::{RecoveryBlocker, RecoveryHalt};
use crate::sync::halt::{build_recovery_halt, persist_pending_config_mutation};
pub use crate::sync::mutation::{ConfigMutation, DependencyUpsertChange};

#[derive(Debug)]
pub struct SyncReport {
    pub applied: ApplyResult,
    pub diagnostics: Vec<Diagnostic>,
    pub dependency_changes: Vec<DependencyUpsertChange>,
    /// What `mars upgrade` would move, for the sync hint.
    pub upgrades: Vec<upgrades::VersionStep>,
    /// Dependencies this run moved to another version or commit.
    pub dependency_moves: Vec<upgrades::VersionStep>,
    pub target_outcomes: Vec<crate::target_sync::TargetSyncOutcome>,
    pub dry_run: bool,
    /// Native harness agent outputs emitted this run that are new or content-changed
    /// vs the previous lock, as `(target_root, dest_path)`. Surfaced so native
    /// emission is not silent in the summary.
    pub native_emitted: Vec<(String, String)>,
    /// Native harness agent outputs removed this run, as `(target_root, dest_path)`.
    /// Surfaced so SuppressAll / selective prunes are not reported as "up to date".
    pub native_removed: Vec<(String, String)>,
    /// Hook files this run wrote (or, in a dry run, would): generated files
    /// (`mars-<name>.hooks.ts`, a native hook written to a file target) and merged ones
    /// (`.codex/hooks.json`), as `(target_root, dest_path)`. Surfaced because a sync
    /// that rewrites only these is not "already up to date".
    pub hook_files_rewritten: Vec<(String, String)>,
    /// Config files of keyed entries (`.mcp.json`, `.codex/config.toml`,
    /// `.agents/plugins.json`, ...) this run wrote or deleted, as `(target_root, file path,
    /// the kind of entry it holds)`. Surfaced for the same reason as `hook_files_rewritten`.
    pub config_files_changed: Vec<(String, String, crate::target::KeyedKind)>,
    /// Present when a recovery command persisted intent but stopped before
    /// materialization because at least one hook surface was unreadable.
    pub recovery_halt: Option<RecoveryHalt>,
    pub engine_fallbacks: Vec<crate::resolve::EngineFallback>,
}

#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub resolution: ResolutionMode,
    pub mutation: Option<ConfigMutation>,
    pub options: SyncOptions,
    /// Whether a recovery command may persist intent and halt when resolution
    /// finds a hook surface the compiler cannot read.
    pub recovery: RecoveryPolicy,
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

#[derive(Debug, Clone)]
pub enum ResolutionMode {
    Normal,
    Maximize {
        targets: HashSet<SourceName>,
        bump: bool,
    },
}

pub(crate) struct LoadedConfig {
    pub config: Config,
    pub local: LocalConfig,
    pub effective: EffectiveConfig,
    pub old_lock: LockFile,
    /// The records of `mars.lock` that serve targets outside this sync, merged back on write.
    pub unscoped: LockFile,
    /// What `mars provide` installed here, read under the sync lock. Never written by sync.
    pub provided: crate::provided::ProvidedNames,
    pub dependency_changes: Vec<DependencyUpsertChange>,
    /// Intentional keepalive — holds the sync file lock for the duration of the pipeline. Dropping this field releases the lock. A dry run holds it only when `.mars/sync.lock` already exists, and never creates it: it mutates nothing the lock guards.
    #[allow(dead_code)]
    pub sync_lock: Option<FileLock>,
}

impl LoadedConfig {
    pub(crate) fn ownership(&self) -> ownership::Ownership<'_> {
        ownership::Ownership::new(&self.old_lock, &self.provided)
    }
}

pub(crate) struct ResolvedState {
    pub loaded: LoadedConfig,
    pub graph: ResolvedGraph,
    pub upgrades: Vec<upgrades::VersionStep>,
    /// Owns every normalized package tree until the compiler has passed its
    /// complete diagnostic gate. Durable staging is published only afterward.
    pub preparation: crate::platform::fs::PendingStagingGeneration,
}

pub(crate) struct TargetedState {
    pub resolved: ResolvedState,
    pub target: TargetState,
}

pub(crate) struct PlannedState {
    pub targeted: TargetedState,
    pub plan: plan::SyncPlan,
}

pub(crate) struct AppliedState {
    pub planned: PlannedState,
    pub applied: ApplyResult,
}

pub(crate) struct SyncedState {
    pub applied: AppliedState,
    pub target_outcomes: Vec<crate::target_sync::TargetSyncOutcome>,
    pub config_entries: BTreeMap<String, BTreeMap<String, crate::lock::ConfigEntryRecord>>,
    pub config_entry_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub rewritten_hook_files: Vec<(String, String)>,
    /// Keyed config files written or deleted this run, as `(target_root, file path, what it holds)`.
    pub config_files_changed: Vec<(String, String, crate::target::KeyedKind)>,
    pub removed_config_entry_outputs: Vec<(String, String)>,
    pub compiled_native_outputs: Vec<crate::lock::CompiledNativeOutput>,
    pub removed_native_outputs: Vec<crate::compiler::RemovedNativeOutput>,
    /// The provided skills this sync projected into targets (`provided::skill_outcomes`).
    pub provided_skills: Vec<apply::ActionOutcome>,
}

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
            upgrades: ir.resolved.upgrades,
            dependency_moves: Vec::new(),
            target_outcomes: Vec::new(),
            dry_run: request.options.dry_run,
            native_emitted: Vec::new(),
            native_removed: Vec::new(),
            hook_files_rewritten: Vec::new(),
            config_files_changed: Vec::new(),
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

    let mut config = match crate::config::load(project_root) {
        Ok(config) => config,
        Err(err) if mutation::is_config_not_found(&err) && request.mutation.is_some() => Config {
            settings: Settings::default(),
            ..Config::default()
        },
        Err(err) => return Err(err),
    };
    let dependency_changes = if let Some(m) = &request.mutation {
        mutation::apply_mutation(&mut config, m)?
    } else {
        Vec::new()
    };

    let mut local = crate::config::load_local(project_root)?;
    if let Some(m) = &request.mutation {
        mutation::apply_local_mutation(&mut local, m);
    }

    let user = crate::config::load_user_config()?;
    let (mut effective, config_diagnostics) =
        crate::config::merge_with_root(config.clone(), local.clone(), &user, project_root)?;
    diag.extend(config_diagnostics);
    // Injected here, not in `merge_with_root`, which publish and the plugin build also call: a
    // host's content belongs to this machine's sync and to nothing it builds or publishes.
    if crate::config::targets::managed_targets(&effective.settings)
        .iter()
        .any(|target| crate::harness::registry::takes_provided(target))
    {
        effective
            .dependencies
            .extend(crate::provided::staged_dependencies(project_root)?);
    }
    diag.extend(
        crate::diagnostic::compatibility_preflight(effective.settings.min_mars_version.as_deref())
            .map_err(|message| crate::error::ConfigError::Invalid { message })?
            .into_iter()
            .collect(),
    );

    if request.options.ignore_requires_mars {
        diag.warn_advisory(
            "requires-mars-disabled",
            "`requires-mars` compatibility checks are disabled by --ignore-requires-mars",
        );
    }
    if request.options.ignore_requires_meridian {
        diag.warn_advisory(
            "requires-meridian-disabled",
            "`requires-meridian` compatibility checks are disabled by --ignore-requires-meridian",
        );
    }
    if let Some(package) = config.package.as_ref() {
        let options = to_resolve_options(&request.resolution, &request.options);
        crate::resolve::check_consumer_package_requirements(package, &options)?;
    }

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

    let managed = crate::config::targets::managed_targets(&effective.settings);
    let (old_lock, unscoped) = old_lock.split_targets(|target| managed.iter().any(|m| m == target));
    diag.extend(crate::config::targets::unnamed_target_diagnostics(
        &unscoped, &config, &local,
    ));
    let provided = crate::provided::ProvidedNames::load(project_root, diag)?;

    Ok(LoadedConfig {
        config,
        local,
        effective,
        old_lock,
        unscoped,
        provided,
        dependency_changes,
        sync_lock,
    })
}

pub(crate) fn resolve_graph(
    ctx: &MarsContext,
    mut loaded: LoadedConfig,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedState, MarsError> {
    let cache = GlobalCache::new()?;
    let source_provider = crate::resolve::RealSourceProvider::new(&cache, &ctx.project_root);
    let source_overrides = loaded.local.source_overrides(&ctx.project_root);
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
    validate_targets(&request.resolution, &graph)?;
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
            diag.warn_advisory(
                "override-missing-dep",
                format!(
                    "override `{override_name}` references a dependency not in the resolved project graph"
                ),
            );
        }
    }
    let upgrades = if request.options.frozen || !request.options.check_upgrades {
        Vec::new()
    } else {
        upgrades::pending_upgrades(
            &graph,
            &loaded.effective,
            &source_provider,
            &resolve_options,
            diag,
        )
    };

    let bump_entries = planned_bump_entries(&loaded.config, &graph, &request.resolution);
    if !bump_entries.is_empty() {
        let bump_changes = mutation::apply_mutation(
            &mut loaded.config,
            &ConfigMutation::BatchUpsert(bump_entries),
        )?;
        loaded.dependency_changes.extend(bump_changes);
    }

    let _ = crate::models::merged_model_aliases(&graph, &loaded.effective, diag);

    Ok(ResolvedState {
        loaded,
        graph,
        upgrades,
        preparation,
    })
}

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
///
/// A dependency hook whose `hook.toml` is invalid keeps what `old_lock` installed; a
/// build with no previous install passes `None` and keeps nothing.
pub(crate) fn build_target_state(
    graph: &crate::resolve::ResolvedGraph,
    effective: &EffectiveConfig,
    local_source: &SourceName,
    old_lock: Option<&crate::lock::LockFile>,
    diag: &mut DiagnosticCollector,
    extend: impl FnOnce(&mut target::TargetState, &mut DiagnosticCollector) -> Result<(), MarsError>,
) -> Result<BuiltTarget, MarsError> {
    let (mut target, explicit_renames, collision_renames, invalid_hooks) =
        target::build_with_collisions_and_diag(graph, effective, local_source, diag)?;
    extend(&mut target, diag)?;
    let managed_targets = crate::config::targets::managed_targets(&effective.settings);
    target.kept_hooks = kept_hooks::settle(
        invalid_hooks,
        &target.items,
        old_lock,
        &managed_targets,
        diag,
    );
    target.resolve_mcp_collisions(&managed_targets, local_source.as_str(), diag)?;
    Ok(BuiltTarget {
        target,
        explicit_renames,
        collision_renames,
    })
}

///
/// Skill refs are checked by each caller after this returns, with its own context: sync knows
/// what this machine installed and what the project's own package declares, a plugin build has
/// neither.
pub(crate) fn apply_target_renames(
    built: BuiltTarget,
    graph: &crate::resolve::ResolvedGraph,
    effective: &EffectiveConfig,
    local_tools: &mars_schema::tool_rule_map::ToolRuleMap,
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
        rewrite::apply_renames(&mut target, &rename_index, graph, &dep_precedence, diag)?;
    }
    package_tools::fold(&mut target, graph, local_tools, diag);

    Ok(PreparedTarget {
        target,
        rename_index,
        explicit_renames,
        collision_renames,
        dep_precedence,
    })
}

pub(crate) fn build_target(
    ctx: &MarsContext,
    resolved: ResolvedState,
    local_items: Vec<crate::local_source::LocalDiscoveredItem>,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<TargetedState, MarsError> {
    let mars_dir = ctx.project_root.join(".mars");
    let local_source = crate::types::SourceOrigin::LocalPackage.to_string().into();

    let mut built = build_target_state(
        &resolved.graph,
        &resolved.loaded.effective,
        &local_source,
        Some(&resolved.loaded.old_lock),
        diag,
        |target_state, diag| {
            local_items::merge_local_items(
                ctx,
                &resolved,
                &local_items,
                request,
                target_state,
                diag,
            )
        },
    )?;

    // Prevent managed installs from overwriting unmanaged files. This runs over
    // every item, hook and MCP directories included, and before renames rewrite
    // references, so a pruned item is neither referenced nor installed. A pruned skill still
    // exists on disk, and the collision says why it is not Mars's: an agent that names it is
    // not also reported as missing it.
    let mut held_back_skills = std::collections::HashSet::new();
    for collision in target::check_unmanaged_collisions(
        &mars_dir,
        &resolved.loaded.ownership(),
        &built.target,
        request.options.force,
    ) {
        // `--force` cannot replace a wrong-shaped destination (a directory where an agent file
        // goes), so name it only where it works.
        let force_works = built.target.items.get(&collision.path).is_none_or(|item| {
            !matches!(
                item.id.kind,
                mars_schema::item::ItemKind::Agent | mars_schema::item::ItemKind::Skill
            ) || local_items::force_adoptable_self_destination(
                &collision.path.resolve(&mars_dir),
                &mars_dir,
                item.id.kind,
            )
        });
        let item = if force_works {
            format!("`{}`", collision.path)
        } else {
            format!(
                "`{}` (`--force` cannot replace it; remove it by hand)",
                collision.path
            )
        };
        diag.warn_each(
            ItemGroup {
                code: "unmanaged-collision",
                subject: collision.source_name.as_str(),
                describe: describe_unmanaged_collisions,
            },
            item,
        );
        if let Some(pruned) = built.target.items.shift_remove(&collision.path)
            && pruned.id.kind == mars_schema::item::ItemKind::Skill
        {
            held_back_skills.insert(pruned.id.name.to_string());
        }
    }

    // The project's own package ships its `.mars-src/` agents; a project with no `[package]`
    // ships none.
    let no_tools = mars_schema::tool_rule_map::ToolRuleMap::default();
    let local_tools = resolved
        .loaded
        .config
        .package
        .as_ref()
        .map_or(&no_tools, |package| &package.tools);
    let PreparedTarget {
        target: target_state,
        explicit_renames: renames,
        collision_renames,
        ..
    } = apply_target_renames(
        built,
        &resolved.graph,
        &resolved.loaded.effective,
        local_tools,
        diag,
    )?;

    validate::validate_skill_refs(
        &target_state,
        &resolved.graph,
        Some(&resolved.loaded.provided),
        resolved
            .loaded
            .config
            .package
            .as_ref()
            .map(|package| package.uses_providers.as_slice()),
        &held_back_skills,
        diag,
    );

    validate::report_skill_links(&target_state, &resolved.graph, &local_items, diag);

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
    })
}

fn describe_disk_divergence(_: &str, count: usize, names: &str) -> String {
    let plural = if count == 1 { "item" } else { "items" };
    format!(
        "{count} installed {plural} in `.mars/` diverged from the mars.lock checksum: {names}. \
         Local content is preserved; run `{}` or `{}` to reset",
        managed_cmd("mars sync --force"),
        managed_cmd("mars repair"),
    )
}

/// One line per source: the paths of a source that Mars did not install but found in `.mars/`.
/// They are the user's own copies, or files Mars wrote and lost track of (a lock that was
/// reset), and `--force` adopts the second kind.
fn describe_unmanaged_collisions(source: &str, count: usize, names: &str) -> String {
    let plural = if count == 1 { "path" } else { "paths" };
    format!(
        "source `{source}` collides with {count} unmanaged {plural} in `.mars/`: {names}. Existing \
         content is left untouched. If it is your own copy, move it out of `.mars/` (what you want \
         to keep belongs in `.mars-src/`). If Mars wrote it and lost track of it, `{}` adopts it, \
         replacing every colliding path in the project and discarding local edits to managed \
         items; preview with `{}`",
        managed_cmd("mars sync --force"),
        managed_cmd("mars sync --force --diff"),
    )
}

pub(crate) fn create_plan(
    ctx: &MarsContext,
    targeted: TargetedState,
    request: &SyncRequest,
    diag: &mut DiagnosticCollector,
) -> Result<PlannedState, MarsError> {
    let mars_dir = ctx.project_root.join(".mars");

    let sync_diff = diff::compute(
        &mars_dir,
        &targeted.resolved.loaded.old_lock,
        &targeted.target,
        request.options.force,
    )?;

    if !request.options.force {
        for entry in &sync_diff.items {
            if let diff::DiffEntry::LocalModified { target, .. } = entry {
                diag.warn_each(
                    ItemGroup {
                        code: "disk-lock-divergent",
                        subject: ".mars",
                        describe: describe_disk_divergence,
                    },
                    format!("`{}`", target.dest_path),
                );
            }
        }
    }

    let sync_plan = plan::create(&sync_diff, &request.options, diag);

    Ok(PlannedState {
        targeted,
        plan: sync_plan,
    })
}

/// A host's hooks and MCP servers change with the host, not with the lock a team commits, so
/// they never trip the gate: a CI machine without the host has none, and one with it is not
/// asking for a lock change.
pub(crate) fn check_frozen_gate(planned: &PlannedState) -> Result<(), MarsError> {
    let has_changes = planned.plan.actions.iter().any(|a| {
        !crate::provided::is_provided_source(a.source_name().as_ref())
            && !matches!(
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

/// Lock is written regardless of target sync outcome.
pub(crate) fn sync_targets(
    ctx: &MarsContext,
    applied: AppliedState,
    request: &SyncRequest,
    agent_surface_policy: crate::compiler::AgentSurfacePolicy,
    diag: &mut DiagnosticCollector,
) -> SyncedState {
    if request.options.dry_run {
        crate::target_sync::preview_native_skill_repairs(
            &ctx.project_root,
            &ctx.project_root.join(".mars"),
            &applied.planned.targeted.resolved.loaded.old_lock,
            &crate::config::targets::managed_targets(
                &applied.planned.targeted.resolved.loaded.effective.settings,
            ),
            &applied.applied.outcomes,
            diag,
        );
        return SyncedState {
            applied,
            target_outcomes: Vec::new(),
            config_entries: BTreeMap::new(),
            config_entry_outputs: Vec::new(),
            rewritten_hook_files: Vec::new(),
            config_files_changed: Vec::new(),
            removed_config_entry_outputs: Vec::new(),
            compiled_native_outputs: Vec::new(),
            removed_native_outputs: Vec::new(),
            provided_skills: Vec::new(),
        };
    }

    let targets = crate::config::targets::managed_targets(
        &applied.planned.targeted.resolved.loaded.effective.settings,
    );
    let target_outcomes = crate::compiler::sync_targets_under_policy(
        &ctx.project_root,
        &targets,
        &applied.applied.outcomes,
        &agent_surface_policy,
        &applied.planned.targeted.resolved.loaded.old_lock,
        request.options.force,
        crate::surface_ownership::CollisionAdoptHint::SyncForce,
        diag,
    );

    SyncedState {
        applied,
        target_outcomes,
        config_entries: BTreeMap::new(),
        config_entry_outputs: Vec::new(),
        rewritten_hook_files: Vec::new(),
        config_files_changed: Vec::new(),
        removed_config_entry_outputs: Vec::new(),
        compiled_native_outputs: Vec::new(),
        removed_native_outputs: Vec::new(),
        provided_skills: Vec::new(),
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

/// An upgrade names any dependency in the resolved graph, direct or required by another one.
fn validate_targets(resolution: &ResolutionMode, graph: &ResolvedGraph) -> Result<(), MarsError> {
    if let ResolutionMode::Maximize { targets, .. } = resolution {
        for name in targets {
            if !graph.nodes.contains_key(name) {
                return Err(mutation::dependency_not_found(
                    graph.nodes.keys(),
                    name,
                    "the resolved project graph",
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
) -> Vec<(SourceName, mars_schema::dependency::Entry)> {
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
            let (mut bumped, _) = mars_schema::dependency::Entry::read(
                name.as_ref(),
                entry.clone(),
                mars_schema::dependency::Strictness::Consumer,
            )
            .ok()?;
            bumped.version = Some(resolved_tag.clone());
            Some((name.clone(), bumped))
        })
        .collect()
}

fn constraint_needs_bump(current: Option<&str>, resolved: &semver::Version) -> bool {
    let constraint = mars_schema::version::parse_constraint(current);
    !mars_schema::version::allows(&[("", &constraint)], resolved, &Default::default())
}

#[cfg(test)]
mod tests;
