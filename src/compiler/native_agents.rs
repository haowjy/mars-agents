//! Native harness agent surfaces: scan, reconcile, compile, and link materialization.

use std::path::Path;

use indexmap::IndexMap;

use super::AgentSurfacePolicy;
use super::agent_copy;
use crate::diagnostic::DiagnosticCollector;
use crate::models::ModelAlias;
use crate::sync::apply::ActionOutcome;

/// Lock output paths removed by native agent reconcile (target_root, dest_path).
pub(crate) type RemovedNativeOutput = (String, String);

pub use crate::lock::CompiledNativeOutput;

/// Inputs for native harness agent reconcile (removals outside target sync).
pub(crate) struct NativeAgentReconcileCtx<'a> {
    pub policy: AgentSurfacePolicy,
    pub project_root: &'a Path,
    pub model_aliases: &'a IndexMap<String, ModelAlias>,
    pub outcomes: &'a [ActionOutcome],
    pub old_lock: &'a crate::lock::LockFile,
    pub dry_run: bool,
    /// When set (e.g. `mars link <target>`), selective reconcile only touches these harnesses.
    pub selective_harness_scope: Option<&'a [crate::compiler::agents::HarnessKind]>,
}

pub(crate) struct NativeAgentSurfaceCompileOptions {
    pub force: bool,
    pub collision_hint: crate::surface_ownership::CollisionAdoptHint,
    pub dry_run: bool,
}

/// Shared inputs for native agent compilation.
pub(crate) struct NativeAgentCompileCtx<'a> {
    pub project_root: &'a Path,
    pub model_aliases: &'a IndexMap<String, ModelAlias>,
    pub cursor_probe_slugs: &'a [String],
    pub old_lock: &'a crate::lock::LockFile,
    pub harness_scope: Option<&'a [crate::compiler::agents::HarnessKind]>,
    pub configured_emit_harnesses: &'a [crate::compiler::agents::HarnessKind],
    pub options: NativeAgentSurfaceCompileOptions,
}

struct NativeAgentEmit<'a> {
    harness: &'a crate::compiler::agents::HarnessKind,
    profile: &'a crate::compiler::agents::AgentProfile,
    fm: &'a crate::frontmatter::Frontmatter,
    body: &'a str,
    agent_name: &'a str,
    canonical_dest_path: &'a str,
    model_override: Option<&'a str>,
}

struct NativeAgentEmitCtx<'a> {
    project_root: &'a Path,
    old_lock: &'a crate::lock::LockFile,
    options: &'a NativeAgentSurfaceCompileOptions,
}

pub(crate) struct MarsCanonicalAgent {
    pub agent_name: String,
    pub canonical_dest_path: String,
    pub profile: crate::compiler::agents::AgentProfile,
    pub fm: crate::frontmatter::Frontmatter,
}

/// Lock-recorded native agent paths to keep during selective target-sync orphan cleanup.
pub fn selective_native_orphan_preserve_paths(
    old_lock: &crate::lock::LockFile,
    spec: &agent_copy::AgentCopySpec,
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};

    let mut preserved: HashMap<String, HashSet<String>> = HashMap::new();
    for harness in &spec.harnesses {
        let target = harness.target_dir();
        for dest_path in old_lock.output_dest_paths_for_target(target) {
            if is_native_agent_dest_path(&dest_path) {
                preserved
                    .entry(target.to_string())
                    .or_default()
                    .insert(dest_path.to_string());
            }
        }
    }
    preserved
}

fn is_native_agent_dest_path(dest_rel: &str) -> bool {
    let Some(name) = dest_rel.strip_prefix("agents/") else {
        return false;
    };
    name.ends_with(".md") || name.ends_with(".toml")
}

pub(crate) fn scan_mars_agents(
    mars_dir: &Path,
    diag: &mut DiagnosticCollector,
) -> Vec<MarsCanonicalAgent> {
    use crate::compiler::agents::parse_agent_content;

    let agents_dir = mars_dir.join("agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return Vec::new();
    };

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                diag.warn(
                    "native-agent-read",
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
                    "native-agent-parse",
                    format!("could not parse {}: {e}", path.display()),
                );
                continue;
            }
        };

        let canonical_file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let canonical_dest_path = format!("agents/{canonical_file_stem}.md");
        let agent_name = profile
            .name
            .as_deref()
            .unwrap_or(&canonical_file_stem)
            .to_string();
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

        agents.push(MarsCanonicalAgent {
            agent_name,
            canonical_dest_path,
            profile,
            fm,
        });
    }

    agents
}

/// Reconcile native harness agent artifacts written outside target sync.
pub(crate) fn reconcile_native_agent_surfaces(
    ctx: &NativeAgentReconcileCtx<'_>,
    mars_agents: &[MarsCanonicalAgent],
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    use crate::lock::ItemKind;

    let mut removed = match &ctx.policy {
        AgentSurfacePolicy::SuppressAll => remove_current_native_agent_surfaces(
            ctx.project_root,
            mars_agents,
            ctx.old_lock,
            ctx.selective_harness_scope,
            ctx.dry_run,
            diag,
        ),
        AgentSurfacePolicy::EmitSelective(spec) => {
            use crate::compiler::agents::HarnessKind;
            let harnesses = match ctx.selective_harness_scope {
                Some(scope) => scope,
                None => HarnessKind::all(),
            };
            reconcile_selective_native_agent_surfaces(ctx, spec, harnesses, mars_agents, diag)
        }
        AgentSurfacePolicy::EmitAll => Vec::new(),
    };

    for outcome in ctx.outcomes {
        if outcome.item_id.kind != ItemKind::Agent
            || !matches!(outcome.action, crate::sync::apply::ActionTaken::Removed)
        {
            continue;
        }

        let agent_name = outcome.dest_path.item_name(ItemKind::Agent);
        removed.extend(remove_native_agent_shapes(
            ctx.project_root,
            &agent_name,
            ctx.old_lock,
            ctx.selective_harness_scope,
            ctx.dry_run,
            diag,
        ));
    }

    removed
}

fn removal_harnesses(
    scope: Option<&[crate::compiler::agents::HarnessKind]>,
) -> &[crate::compiler::agents::HarnessKind] {
    match scope {
        Some(harnesses) => harnesses,
        None => crate::compiler::agents::HarnessKind::all(),
    }
}

fn remove_current_native_agent_surfaces(
    project_root: &Path,
    mars_agents: &[MarsCanonicalAgent],
    old_lock: &crate::lock::LockFile,
    harness_scope: Option<&[crate::compiler::agents::HarnessKind]>,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    let mut removed = Vec::new();
    for agent in mars_agents {
        removed.extend(remove_native_agent_shapes(
            project_root,
            &agent.agent_name,
            old_lock,
            harness_scope,
            dry_run,
            diag,
        ));
    }
    removed
}

fn remove_native_agent_shapes(
    project_root: &Path,
    agent_name: &str,
    old_lock: &crate::lock::LockFile,
    harness_scope: Option<&[crate::compiler::agents::HarnessKind]>,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    let mut removed = Vec::new();
    for harness in removal_harnesses(harness_scope) {
        removed.extend(remove_native_agent_shapes_for_harness(
            project_root,
            agent_name,
            harness,
            old_lock,
            dry_run,
            diag,
        ));
    }
    removed
}

fn reconcile_selective_native_agent_surfaces(
    ctx: &NativeAgentReconcileCtx<'_>,
    spec: &agent_copy::AgentCopySpec,
    harnesses: &[crate::compiler::agents::HarnessKind],
    mars_agents: &[MarsCanonicalAgent],
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    let mut removed = Vec::new();
    for agent in mars_agents {
        // `agent.profile` is already overlay-resolved (see the lifecycle), so reconcile
        // and emission qualify against identical effective profiles.
        for harness in harnesses {
            let qualifies = spec.harnesses.contains(harness)
                && agent_copy::agent_qualifies_for_harness(
                    &agent.profile,
                    harness,
                    ctx.model_aliases,
                    spec.include_fanout,
                )
                .is_some();
            if qualifies {
                continue;
            }
            removed.extend(remove_native_agent_shapes_for_harness(
                ctx.project_root,
                &agent.agent_name,
                harness,
                ctx.old_lock,
                ctx.dry_run,
                diag,
            ));
        }
    }
    removed
}

fn remove_native_agent_shapes_for_harness(
    project_root: &Path,
    agent_name: &str,
    harness: &crate::compiler::agents::HarnessKind,
    old_lock: &crate::lock::LockFile,
    dry_run: bool,
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    let mut removed = Vec::new();
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
        let absent = !native_path.exists() && native_path.symlink_metadata().is_err();
        if absent {
            removed.push((target.to_string(), dest_rel));
            continue;
        }
        if dry_run {
            continue;
        }
        match crate::reconcile::fs_ops::safe_remove(&native_path) {
            Ok(()) => removed.push((target.to_string(), dest_rel)),
            Err(e) => diag.warn(
                "native-agent-remove",
                format!("could not remove {}: {e}", native_path.display()),
            ),
        }
    }
    removed
}

/// Compile native harness agents from a pre-scanned canonical store.
pub(crate) fn compile_native_agents(
    ctx: &NativeAgentCompileCtx<'_>,
    policy: &AgentSurfacePolicy,
    mars_agents: &[MarsCanonicalAgent],
    diag: &mut DiagnosticCollector,
) -> Vec<CompiledNativeOutput> {
    if matches!(policy, AgentSurfacePolicy::SuppressAll) {
        return Vec::new();
    }

    let emit_ctx = NativeAgentEmitCtx {
        project_root: ctx.project_root,
        old_lock: ctx.old_lock,
        options: &ctx.options,
    };
    let mut records = Vec::new();

    // `mars_agents` carry already overlay-resolved profiles (resolved once in
    // run_native_agent_post_sync_lifecycle for both reconcile and compile).
    for agent in mars_agents {
        let effective_profile = &agent.profile;
        for (harness, directive) in qualifying_emissions(effective_profile, policy, ctx, diag) {
            // `Clear` emits to a harness the agent's model does not resolve to;
            // strip the model so lowering writes no model field.
            let cleared_profile;
            let (profile, model_override): (&crate::compiler::agents::AgentProfile, Option<&str>) =
                match &directive {
                    NativeModelDirective::Resolved(model) => (effective_profile, model.as_deref()),
                    NativeModelDirective::Clear => {
                        cleared_profile = {
                            let mut p = effective_profile.clone();
                            p.model = None;
                            p
                        };
                        (&cleared_profile, None)
                    }
                };
            emit_lowered_native_agent(
                &NativeAgentEmit {
                    harness: &harness,
                    profile,
                    fm: &agent.fm,
                    body: agent.fm.body(),
                    agent_name: &agent.agent_name,
                    canonical_dest_path: &agent.canonical_dest_path,
                    model_override,
                },
                &emit_ctx,
                diag,
                &mut records,
            );
        }
    }

    records
}

/// Merge a per-agent overlay's model selection over the canonical profile for native
/// emission: `overlay.model` wins over `profile.model`, and overlay policies take
/// precedence over profile policies (shared with launch-bundle via the config helpers).
/// `overlay.harness` is intentionally ignored — native coverage is configured-target
/// driven, not overlay-routed.
fn effective_native_profile(
    profile: &crate::compiler::agents::AgentProfile,
    overlay: Option<&crate::config::AgentOverlay>,
) -> crate::compiler::agents::AgentProfile {
    let Some(overlay) = overlay else {
        return profile.clone();
    };
    let mut effective = profile.clone();
    effective.model =
        crate::config::overlay_then_profile_model(Some(overlay), profile.model.as_deref())
            .map(|model| model.to_string());
    effective.model_policies =
        crate::config::overlay_then_profile_policies(Some(overlay), &profile.model_policies)
            .map(|(_, _, rule)| rule.clone())
            .collect();
    effective
}

/// What model field a native emission should carry.
///
/// `Option<String>` alone is two-state ("pin this id" vs "fall back to the
/// profile's own model"); full-coverage `EmitAll` needs a third state that
/// clears the model entirely when an agent is emitted to a harness whose model
/// it does not resolve to.
#[derive(Debug, Clone)]
enum NativeModelDirective {
    /// Qualified emission: pin `Some(id)`, or `None` to emit the profile's own
    /// model verbatim (e.g. an unpinned alias or a fanout token).
    Resolved(Option<String>),
    /// Full-coverage emission to a non-resolving harness: emit no model field.
    Clear,
}

fn qualifying_emissions(
    profile: &crate::compiler::agents::AgentProfile,
    policy: &AgentSurfacePolicy,
    ctx: &NativeAgentCompileCtx<'_>,
    diag: &mut DiagnosticCollector,
) -> Vec<(crate::compiler::agents::HarnessKind, NativeModelDirective)> {
    use crate::compiler::agents::HarnessKind;

    let in_scope = |harness: &HarnessKind| {
        ctx.harness_scope
            .is_none_or(|scope| scope.contains(harness))
    };

    match policy {
        AgentSurfacePolicy::SuppressAll => Vec::new(),
        AgentSurfacePolicy::EmitAll => {
            let mut emissions = Vec::new();
            for harness in ctx.configured_emit_harnesses {
                if !in_scope(harness) {
                    continue;
                }
                let directive = match agent_copy::agent_qualifies_for_harness(
                    profile,
                    harness,
                    ctx.model_aliases,
                    true,
                ) {
                    Some(emission) => NativeModelDirective::Resolved(model_override_for_emission(
                        harness,
                        profile,
                        &emission,
                        ctx.model_aliases,
                        ctx.cursor_probe_slugs,
                        diag,
                    )),
                    None => NativeModelDirective::Clear,
                };
                emissions.push((harness.clone(), directive));
            }
            emissions
        }
        AgentSurfacePolicy::EmitSelective(spec) => {
            let mut emissions = Vec::new();
            for harness in &spec.harnesses {
                if !in_scope(harness) {
                    continue;
                }
                let Some(emission) = agent_copy::agent_qualifies_for_harness(
                    profile,
                    harness,
                    ctx.model_aliases,
                    spec.include_fanout,
                ) else {
                    continue;
                };
                let directive = NativeModelDirective::Resolved(model_override_for_emission(
                    harness,
                    profile,
                    &emission,
                    ctx.model_aliases,
                    ctx.cursor_probe_slugs,
                    diag,
                ));
                emissions.push((harness.clone(), directive));
            }
            emissions
        }
    }
}

fn emit_lowered_native_agent(
    agent: &NativeAgentEmit<'_>,
    ctx: &NativeAgentEmitCtx<'_>,
    diag: &mut DiagnosticCollector,
    records: &mut Vec<CompiledNativeOutput>,
) {
    use crate::compiler::agents::lower::lower_for_harness_with_model;
    use crate::surface_ownership::{self, SurfaceCopyDecision};

    let lowered = lower_for_harness_with_model(
        agent.harness,
        agent.profile,
        agent.fm,
        agent.body,
        agent.model_override,
    );

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
        records.push(CompiledNativeOutput {
            owner_canonical_dest_path: agent.canonical_dest_path.to_string(),
            target_root: target_dir.to_string(),
            dest_path: dest_rel,
            installed_checksum: checksum,
        });
    }
}

pub(crate) fn merged_model_aliases_for_native_agents(
    resolved: &crate::sync::ResolvedState,
) -> IndexMap<String, ModelAlias> {
    let mut local_diag = DiagnosticCollector::new();
    crate::models::merged_model_aliases(
        &resolved.graph,
        &resolved.loaded.effective,
        &resolved.loaded.config,
        &resolved.loaded.local,
        &mut local_diag,
    )
}

pub(crate) fn cached_cursor_probe_slugs_for_native_agents() -> Vec<String> {
    crate::models::probes::cursor_cache::read_cached_probe_result_usable()
        .map(|probe| probe.slugs)
        .unwrap_or_default()
}

pub(crate) fn native_model_override_for_harness(
    harness: &crate::compiler::agents::HarnessKind,
    profile: &crate::compiler::agents::AgentProfile,
    aliases: &IndexMap<String, ModelAlias>,
    cursor_probe_slugs: &[String],
    diag: &mut DiagnosticCollector,
) -> Option<String> {
    let token = profile.model.as_deref()?;
    if matches!(harness, crate::compiler::agents::HarnessKind::Cursor) {
        return map_cursor_native_model(profile, aliases, cursor_probe_slugs);
    }
    if token.contains('[') {
        return None;
    }
    let alias = aliases.get(token)?;
    if let Some(pinned) = alias.pinned_model_id() {
        return Some(pinned.to_string());
    }
    diag.warn(
        "native-model-alias-unpinned",
        format!(
            "native agent compile: alias `{token}` has no pinned model id for {}; emitting alias verbatim",
            harness.target_dir()
        ),
    );
    None
}

pub(crate) fn model_override_for_emission(
    harness: &crate::compiler::agents::HarnessKind,
    profile: &crate::compiler::agents::AgentProfile,
    emission: &agent_copy::QualifiedEmission,
    model_aliases: &IndexMap<String, ModelAlias>,
    cursor_probe_slugs: &[String],
    diag: &mut DiagnosticCollector,
) -> Option<String> {
    match emission {
        agent_copy::QualifiedEmission::DefaultModel => native_model_override_for_harness(
            harness,
            profile,
            model_aliases,
            cursor_probe_slugs,
            diag,
        ),
        agent_copy::QualifiedEmission::PolicyModel(token) => {
            let mut profile_for_policy = profile.clone();
            profile_for_policy.model = Some(token.clone());
            native_model_override_for_harness(
                harness,
                &profile_for_policy,
                model_aliases,
                cursor_probe_slugs,
                diag,
            )
            .or_else(|| Some(token.clone()))
        }
    }
}

fn map_cursor_native_model(
    profile: &crate::compiler::agents::AgentProfile,
    aliases: &IndexMap<String, ModelAlias>,
    cursor_probe_slugs: &[String],
) -> Option<String> {
    let token = profile.model.as_deref()?;
    if token.contains('[') {
        return None;
    }

    let alias = aliases.get(token);
    let model_id = alias.and_then(|a| a.pinned_model_id()).unwrap_or(token);
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

fn cursor_effective_effort<'a>(
    profile: &'a crate::compiler::agents::AgentProfile,
    alias: Option<&'a ModelAlias>,
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

/// Inputs for native harness agent materialization after `mars link`.
pub(crate) struct NativeAgentLinkMaterializeCtx<'a> {
    pub mars_ctx: &'a crate::types::MarsContext,
    pub managed_targets: &'a [String],
    pub config: &'a crate::config::Config,
    pub local: &'a crate::config::LocalConfig,
    pub effective: &'a crate::config::EffectiveConfig,
    pub graph: &'a crate::resolve::ResolvedGraph,
    pub old_lock: &'a crate::lock::LockFile,
    pub target_outcomes: &'a [crate::target_sync::TargetSyncOutcome],
    pub force: bool,
}

/// Reconcile native harness agents, then compile when policy allows (shared sync/link path).
pub(crate) fn run_native_agent_post_sync_lifecycle(
    reconcile_ctx: &NativeAgentReconcileCtx<'_>,
    policy: &AgentSurfacePolicy,
    mars_agents: &[MarsCanonicalAgent],
    agent_overlays: &indexmap::IndexMap<String, crate::config::AgentOverlay>,
    compile_ctx: Option<&NativeAgentCompileCtx<'_>>,
    diag: &mut DiagnosticCollector,
) -> (Vec<CompiledNativeOutput>, Vec<RemovedNativeOutput>) {
    // Resolve per-agent overlays into effective profiles ONCE here, so reconcile and
    // compile qualify against identical state without each re-deriving it (and without
    // threading the overlay map through their contexts).
    let resolved = resolve_native_agent_profiles(mars_agents, agent_overlays);
    let removed_native_outputs = reconcile_native_agent_surfaces(reconcile_ctx, &resolved, diag);
    let compiled_native_outputs = match compile_ctx {
        None => Vec::new(),
        Some(ctx) => compile_native_agents(ctx, policy, &resolved, diag),
    };
    (compiled_native_outputs, removed_native_outputs)
}

/// Apply each agent's overlay (`overlay.model` / `overlay.model_policies`) over its
/// canonical profile, yielding agents whose `profile` is the effective native profile.
/// Single source of overlay merging for the native lifecycle.
fn resolve_native_agent_profiles(
    mars_agents: &[MarsCanonicalAgent],
    agent_overlays: &indexmap::IndexMap<String, crate::config::AgentOverlay>,
) -> Vec<MarsCanonicalAgent> {
    mars_agents
        .iter()
        .map(|agent| MarsCanonicalAgent {
            agent_name: agent.agent_name.clone(),
            canonical_dest_path: agent.canonical_dest_path.clone(),
            profile: effective_native_profile(
                &agent.profile,
                agent_overlays.get(&agent.agent_name),
            ),
            fm: agent.fm.clone(),
        })
        .collect()
}

/// Reconcile and compile native harness agents after `mars link` (same path as sync).
pub(crate) fn materialize_native_agents_after_link(
    input: &NativeAgentLinkMaterializeCtx<'_>,
    diag: &mut DiagnosticCollector,
) -> (Vec<CompiledNativeOutput>, Vec<RemovedNativeOutput>) {
    use crate::compiler::agents::HarnessKind;

    if !input
        .managed_targets
        .iter()
        .any(|target| HarnessKind::from_target_dir(target).is_some())
    {
        return (Vec::new(), Vec::new());
    }

    let link_harness_scope: Vec<_> = input
        .target_outcomes
        .iter()
        .filter_map(|outcome| HarnessKind::from_target_dir(&outcome.target))
        .collect();
    if link_harness_scope.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let agent_copy_spec = agent_copy::build_agent_copy_spec(
        input.effective.settings.meridian_agent_copy(),
        input.managed_targets,
        diag,
    );
    let policy = super::agent_surface_policy(
        input.effective.settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        input.mars_ctx.meridian_managed,
    );
    let mars_dir = input.mars_ctx.project_root.join(".mars");
    let mars_agents = scan_mars_agents(&mars_dir, diag);
    let model_aliases = crate::models::merged_model_aliases(
        input.graph,
        input.effective,
        input.config,
        input.local,
        diag,
    );
    // Per-agent overlays merged from mars.toml + mars.local.toml (link path lacks the
    // project-level effective.agents map the sync path carries).
    let agent_overlays = crate::config::merged_agent_overlays(&input.config.agents, input.local);
    let harness_scope = Some(link_harness_scope.as_slice());
    let configured_emit_harnesses: Vec<_> = input
        .managed_targets
        .iter()
        .filter_map(|t| HarnessKind::from_target_dir(t))
        .collect();
    let reconcile_ctx = NativeAgentReconcileCtx {
        policy: policy.clone(),
        project_root: &input.mars_ctx.project_root,
        model_aliases: &model_aliases,
        outcomes: &[],
        old_lock: input.old_lock,
        dry_run: false,
        selective_harness_scope: harness_scope,
    };
    let ownership_lock;
    let compile_ctx = if matches!(policy, AgentSurfacePolicy::SuppressAll) {
        None
    } else {
        ownership_lock =
            crate::lock::ownership_lock_after_target_sync(input.old_lock, input.target_outcomes);
        Some(NativeAgentCompileCtx {
            project_root: &input.mars_ctx.project_root,
            model_aliases: &model_aliases,
            cursor_probe_slugs: &cached_cursor_probe_slugs_for_native_agents(),
            old_lock: &ownership_lock,
            harness_scope,
            configured_emit_harnesses: &configured_emit_harnesses,
            options: NativeAgentSurfaceCompileOptions {
                force: input.force,
                collision_hint: crate::surface_ownership::CollisionAdoptHint::LinkForce,
                dry_run: false,
            },
        })
    };
    let (compiled_native_outputs, removed_native_outputs) = run_native_agent_post_sync_lifecycle(
        &reconcile_ctx,
        &policy,
        &mars_agents,
        &agent_overlays,
        compile_ctx.as_ref(),
        diag,
    );
    (compiled_native_outputs, removed_native_outputs)
}

#[cfg(test)]
mod tests;
