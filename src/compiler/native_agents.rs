//! Native harness agent surfaces: scan, reconcile, compile, and link materialization.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use indexmap::IndexMap;

use super::AgentSurfacePolicy;
use super::agent_copy;
use crate::config::ModelPolicyMatchType;
use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::host::{CapabilityCollectionOptions, CapabilitySession};
use crate::models::{ModelAlias, ModelSpec, ModelsCache};
use crate::sync::apply::ActionOutcome;

/// Lock output paths removed by native agent reconcile (target_root, dest_path).
pub(crate) type RemovedNativeOutput = (String, String);

pub use crate::lock::CompiledNativeOutput;

/// Inputs for native harness agent reconcile (removals outside target sync).
pub(crate) struct NativeAgentReconcileCtx<'a> {
    pub policy: AgentSurfacePolicy,
    pub project_root: &'a Path,
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
    model: &'a crate::compiler::agents::lower::NativeModel,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeModelDecision {
    Set { model_id: String },
    Clear,
    Skip,
}

#[derive(Debug, Clone)]
struct NativeResolvedModel<'a> {
    model_id: String,
    provider_for_order: Option<String>,
    provider_constraint: Option<String>,
    alias: Option<&'a ModelAlias>,
}

#[derive(Debug, Clone)]
struct NativeModelCandidate {
    token: String,
    harness_constraint: Option<crate::compiler::agents::HarnessKind>,
}

struct NativeSessionProbeResolver<'a> {
    session: &'a mut CapabilitySession,
}

impl crate::routing::ProbeResolver for NativeSessionProbeResolver<'_> {
    fn opencode_probe_result(&mut self) -> Option<crate::models::probes::OpenCodeProbeResult> {
        self.session.opencode_probe_result()
    }

    fn pi_probe_result(&mut self) -> Option<crate::models::probes::PiProbeResult> {
        self.session.pi_probe_result()
    }

    fn cursor_probe_result(&mut self) -> Option<crate::models::probes::CursorProbeResult> {
        self.session.cursor_probe_result()
    }
}

/// Command-scoped native model router. Native surfaces must emit one concrete
/// harness model per file, but the accept/reject decision is delegated to the
/// same routing evaluator used by `mars models resolve`.
pub(crate) struct NativeModelRoutingRuntime<'a> {
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &'a ModelsCache,
    catalog_model_slugs: Vec<String>,
    routing_settings: ResolvedRoutingSettings,
    session: CapabilitySession,
    installed_for_native_targets: HashSet<String>,
    memo: HashMap<(String, String, Option<String>), Option<String>>,
}

impl<'a> NativeModelRoutingRuntime<'a> {
    pub(crate) fn collect(
        aliases: &'a IndexMap<String, ModelAlias>,
        cache: &'a ModelsCache,
        routing_settings: ResolvedRoutingSettings,
    ) -> Self {
        let session = CapabilitySession::collect_without_auth(&CapabilityCollectionOptions {
            offline: crate::models::is_mars_offline(),
            probe_refresh: crate::models::probes::ProbeRefreshMode::Background,
        });
        Self::with_session(aliases, cache, routing_settings, session)
    }

    fn with_session(
        aliases: &'a IndexMap<String, ModelAlias>,
        cache: &'a ModelsCache,
        routing_settings: ResolvedRoutingSettings,
        session: CapabilitySession,
    ) -> Self {
        let mut installed_for_native_targets = session.installed_harnesses();
        installed_for_native_targets.extend(
            crate::compiler::agents::HarnessKind::all()
                .iter()
                .map(|harness| harness.to_harness_id().as_str().to_string()),
        );
        Self {
            aliases,
            cache,
            catalog_model_slugs: crate::models::catalog_model_slugs(cache),
            routing_settings,
            session,
            installed_for_native_targets,
            memo: HashMap::new(),
        }
    }

    pub(crate) fn decision_for_profile(
        &mut self,
        profile: &crate::compiler::agents::AgentProfile,
        target_harness: &crate::compiler::agents::HarnessKind,
        include_fanout: bool,
        emit_all: bool,
    ) -> NativeModelDecision {
        for candidate in self.candidates(profile, include_fanout) {
            if candidate
                .harness_constraint
                .as_ref()
                .is_some_and(|constraint| constraint != target_harness)
            {
                continue;
            }
            if let Some(model_id) = self.route_candidate(profile, &candidate.token, target_harness)
            {
                return NativeModelDecision::Set { model_id };
            }
        }

        if emit_all {
            NativeModelDecision::Clear
        } else {
            NativeModelDecision::Skip
        }
    }

    fn candidates(
        &self,
        profile: &crate::compiler::agents::AgentProfile,
        include_fanout: bool,
    ) -> Vec<NativeModelCandidate> {
        let mut candidates = Vec::new();
        if let Some(model) = profile.model.as_deref()
            && !model.trim().is_empty()
        {
            candidates.push(NativeModelCandidate {
                token: model.trim().to_string(),
                harness_constraint: None,
            });
        }
        if include_fanout {
            for policy in &profile.model_policies {
                candidates.extend(self.policy_candidates(policy));
            }
        }
        candidates
    }

    fn policy_candidates(
        &self,
        policy: &crate::config::ModelPolicyRule,
    ) -> Vec<NativeModelCandidate> {
        let value = policy.match_value.trim();
        if value.is_empty() {
            return Vec::new();
        }
        let harness_constraint = policy_override_harness(policy);

        match policy.match_type {
            ModelPolicyMatchType::Alias | ModelPolicyMatchType::Model => {
                vec![NativeModelCandidate {
                    token: value.to_string(),
                    harness_constraint,
                }]
            }
            ModelPolicyMatchType::ModelGlob => self
                .cache
                .models
                .iter()
                .filter(|model| crate::models::glob_match(value, &model.id))
                .map(|model| NativeModelCandidate {
                    token: model.id.clone(),
                    harness_constraint: harness_constraint.clone(),
                })
                .collect(),
        }
    }

    fn route_candidate(
        &mut self,
        profile: &crate::compiler::agents::AgentProfile,
        token: &str,
        target_harness: &crate::compiler::agents::HarnessKind,
    ) -> Option<String> {
        let memo_effort = if *target_harness == crate::compiler::agents::HarnessKind::Cursor {
            self.resolve_candidate(token).and_then(|resolved| {
                cursor_effective_effort(profile, resolved.alias).map(str::to_string)
            })
        } else {
            None
        };
        let key = (
            token.to_string(),
            target_harness.to_harness_id().as_str().to_string(),
            memo_effort,
        );
        if let Some(cached) = self.memo.get(&key) {
            return cached.clone();
        }

        let routed = self.route_candidate_uncached(profile, token, target_harness);
        self.memo.insert(key, routed.clone());
        routed
    }

    fn route_candidate_uncached(
        &mut self,
        profile: &crate::compiler::agents::AgentProfile,
        token: &str,
        target_harness: &crate::compiler::agents::HarnessKind,
    ) -> Option<String> {
        let resolved = self.resolve_candidate(token)?;
        let target_name = target_harness.to_harness_id().as_str().to_string();
        let linked_harnesses = [target_name.clone()];
        let provider_order = self.routing_settings.provider_order_names();
        let harness_order = self.routing_settings.harness_order_names();
        let default_harness = self.routing_settings.default_harness_name();
        let route_model_ids = self.route_model_ids(target_harness, &resolved.model_id);

        for route_model_id in route_model_ids {
            let input = crate::routing::RoutingInput {
                model_id: &route_model_id,
                provider_for_order: resolved.provider_for_order.as_deref(),
                provider_constraint: resolved.provider_constraint.as_deref(),
                settings_provider_order: provider_order.as_deref(),
                settings_harness_order: harness_order.as_deref(),
                config_default_harness: default_harness.as_deref(),
                installed_harnesses: &self.installed_for_native_targets,
                linked_harnesses: Some(linked_harnesses.as_slice()),
                opencode_probe_result: None,
                pi_probe_result: None,
                cursor_probe_result: None,
                catalog_model_slugs: Some(self.catalog_model_slugs.as_slice()),
            };
            let mut probe_resolver = NativeSessionProbeResolver {
                session: &mut self.session,
            };
            let trace = crate::routing::evaluate_candidates_with_auth_and_probes(
                &input,
                &mut probe_resolver,
                |_| true,
            );
            if trace.selected_harness() != target_name {
                continue;
            }
            if matches!(
                trace.selected_selection_kind(),
                crate::routing::SelectionKind::ConfigDefault
                    | crate::routing::SelectionKind::LinkedFallback
            ) {
                continue;
            }
            if crate::routing::acceptance::accept_route(
                &trace,
                &self.installed_for_native_targets,
                native_acceptance_policy(target_harness),
            )
            .is_err()
            {
                continue;
            }

            return Some(self.native_model_id(profile, target_harness, &resolved, &route_model_id));
        }
        None
    }

    fn route_model_ids(
        &self,
        target_harness: &crate::compiler::agents::HarnessKind,
        model_id: &str,
    ) -> Vec<String> {
        let mut ids = vec![model_id.to_string()];
        if *target_harness == crate::compiler::agents::HarnessKind::Cursor
            && let Some(shimmed) = cursor_probe_model_id_shim(model_id)
            && !ids.iter().any(|id| id == &shimmed)
        {
            ids.push(shimmed);
        }
        ids
    }

    fn native_model_id(
        &mut self,
        profile: &crate::compiler::agents::AgentProfile,
        target_harness: &crate::compiler::agents::HarnessKind,
        resolved: &NativeResolvedModel<'_>,
        routed_model_id: &str,
    ) -> String {
        if *target_harness != crate::compiler::agents::HarnessKind::Cursor {
            return resolved.model_id.clone();
        }

        let effort = cursor_effective_effort(profile, resolved.alias).unwrap_or("medium");
        let Some(cursor_probe) = self.session.cursor_probe_result() else {
            return routed_model_id.to_string();
        };
        crate::models::probes::cursor::resolve_cursor_effort_slug(
            routed_model_id,
            effort,
            &cursor_probe.slugs,
        )
        .map(|resolution| resolution.slug)
        .unwrap_or_else(|_| routed_model_id.to_string())
    }

    fn resolve_candidate(&self, token: &str) -> Option<NativeResolvedModel<'a>> {
        let alias = self.aliases.get(token);
        let (raw_model_token, token_provider_constraint) =
            crate::models::split_provider_constrained_model_token(token);
        let model_id = match alias {
            Some(alias) => crate::models::resolve_model_id_for_alias(alias, self.cache)?,
            None => raw_model_token.clone(),
        };
        if model_id.trim().is_empty() {
            return None;
        }

        let provider_constraint = alias
            .and_then(provider_constraint_for_alias)
            .or(token_provider_constraint.clone());
        let provider_for_order = alias
            .and_then(|alias| crate::models::resolve_provider_for_alias(alias, self.cache))
            .or_else(|| {
                token_provider_constraint.or_else(|| {
                    crate::models::infer_provider_from_model_id(&model_id).map(str::to_string)
                })
            });

        Some(NativeResolvedModel {
            model_id,
            provider_for_order,
            provider_constraint,
            alias,
        })
    }
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

fn reconcile_native_agent_surfaces_without_model_routing(
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
        AgentSurfacePolicy::EmitSelective(_) | AgentSurfacePolicy::EmitAll => Vec::new(),
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

/// Reconcile native harness agent artifacts written outside target sync.
pub(crate) fn reconcile_native_agent_surfaces(
    ctx: &NativeAgentReconcileCtx<'_>,
    mars_agents: &[MarsCanonicalAgent],
    model_router: &mut NativeModelRoutingRuntime<'_>,
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
            reconcile_selective_native_agent_surfaces(
                ctx,
                spec,
                harnesses,
                mars_agents,
                model_router,
                diag,
            )
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
    model_router: &mut NativeModelRoutingRuntime<'_>,
    diag: &mut DiagnosticCollector,
) -> Vec<RemovedNativeOutput> {
    let mut removed = Vec::new();
    for agent in mars_agents {
        // `agent.profile` is already overlay-resolved (see the lifecycle), so reconcile
        // and emission qualify against identical effective profiles.
        for harness in harnesses {
            let qualifies = spec.harnesses.contains(harness)
                && matches!(
                    model_router.decision_for_profile(
                        &agent.profile,
                        harness,
                        spec.include_fanout,
                        false,
                    ),
                    NativeModelDecision::Set { .. }
                );
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
pub(crate) fn compile_native_agents<'a>(
    ctx: &NativeAgentCompileCtx<'_>,
    policy: &AgentSurfacePolicy,
    mars_agents: impl IntoIterator<Item = &'a MarsCanonicalAgent>,
    model_router: &mut NativeModelRoutingRuntime<'_>,
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
        for (harness, model) in qualifying_emissions(effective_profile, policy, ctx, model_router) {
            emit_lowered_native_agent(
                &NativeAgentEmit {
                    harness: &harness,
                    profile: effective_profile,
                    fm: &agent.fm,
                    body: agent.fm.body(),
                    agent_name: &agent.agent_name,
                    canonical_dest_path: &agent.canonical_dest_path,
                    model: &model,
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

fn qualifying_emissions(
    profile: &crate::compiler::agents::AgentProfile,
    policy: &AgentSurfacePolicy,
    ctx: &NativeAgentCompileCtx<'_>,
    model_router: &mut NativeModelRoutingRuntime<'_>,
) -> Vec<(
    crate::compiler::agents::HarnessKind,
    crate::compiler::agents::lower::NativeModel,
)> {
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
                let model = match model_router.decision_for_profile(profile, harness, true, true) {
                    NativeModelDecision::Set { model_id } => {
                        crate::compiler::agents::lower::NativeModel::Set(model_id)
                    }
                    NativeModelDecision::Clear | NativeModelDecision::Skip => {
                        crate::compiler::agents::lower::NativeModel::Clear
                    }
                };
                emissions.push((harness.clone(), model));
            }
            emissions
        }
        AgentSurfacePolicy::EmitSelective(spec) => {
            let mut emissions = Vec::new();
            for harness in &spec.harnesses {
                if !in_scope(harness) {
                    continue;
                }
                if let NativeModelDecision::Set { model_id } =
                    model_router.decision_for_profile(profile, harness, spec.include_fanout, false)
                {
                    emissions.push((
                        harness.clone(),
                        crate::compiler::agents::lower::NativeModel::Set(model_id),
                    ));
                }
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
        agent.model,
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

fn provider_constraint_for_alias(alias: &ModelAlias) -> Option<String> {
    match &alias.spec {
        ModelSpec::Pinned { provider, .. } | ModelSpec::PinnedWithMatch { provider, .. } => {
            provider.clone()
        }
        ModelSpec::AutoResolve { provider, .. } => provider.clone(),
    }
    .map(|provider| provider.trim().to_ascii_lowercase())
    .filter(|provider| !provider.is_empty())
}

fn policy_override_harness(
    policy: &crate::config::ModelPolicyRule,
) -> Option<crate::compiler::agents::HarnessKind> {
    policy
        .overrides
        .get(serde_yaml::Value::String("harness".to_string()))
        .and_then(|value| value.as_str())
        .and_then(crate::compiler::agents::HarnessKind::from_str)
}

fn native_acceptance_policy(
    target_harness: &crate::compiler::agents::HarnessKind,
) -> crate::routing::acceptance::MatchPolicy {
    match target_harness {
        crate::compiler::agents::HarnessKind::Claude
        | crate::compiler::agents::HarnessKind::Codex => {
            crate::routing::acceptance::MatchPolicy::AllowPassthrough
        }
        crate::compiler::agents::HarnessKind::OpenCode
        | crate::compiler::agents::HarnessKind::Cursor
        | crate::compiler::agents::HarnessKind::Pi => {
            crate::routing::acceptance::MatchPolicy::RequireSlugEvidence
        }
    }
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
    mut model_router: Option<&mut NativeModelRoutingRuntime<'_>>,
    diag: &mut DiagnosticCollector,
) -> (Vec<CompiledNativeOutput>, Vec<RemovedNativeOutput>) {
    // Resolve per-agent overlays into effective profiles ONCE here, so reconcile and
    // compile qualify against identical state without each re-deriving it (and without
    // threading the overlay map through their contexts).
    let resolved = resolve_native_agent_profiles(mars_agents, agent_overlays);
    let removed_native_outputs = match reconcile_ctx.policy {
        AgentSurfacePolicy::EmitSelective(_) => reconcile_native_agent_surfaces(
            reconcile_ctx,
            &resolved,
            model_router
                .as_deref_mut()
                .expect("native model router required for selective native reconcile"),
            diag,
        ),
        AgentSurfacePolicy::SuppressAll | AgentSurfacePolicy::EmitAll => {
            reconcile_native_agent_surfaces_without_model_routing(reconcile_ctx, &resolved, diag)
        }
    };
    let compiled_native_outputs = match compile_ctx {
        None => Vec::new(),
        Some(ctx) => compile_native_agents(
            ctx,
            policy,
            resolved.iter().filter(|agent| {
                ctx.old_lock.contains_output(
                    crate::lock::CANONICAL_TARGET_ROOT,
                    &agent.canonical_dest_path,
                )
            }),
            model_router
                .as_deref_mut()
                .expect("native model router required for native compile"),
            diag,
        ),
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
    let models_cache =
        crate::models::read_cache(&mars_dir).unwrap_or_else(|_| crate::models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        });
    let model_aliases = crate::models::merged_model_aliases(
        input.graph,
        input.effective,
        input.config,
        input.local,
        diag,
    );
    let routing_settings = ResolvedRoutingSettings::from_settings(&input.effective.settings);
    let mut model_router = (!matches!(policy, AgentSurfacePolicy::SuppressAll)).then(|| {
        NativeModelRoutingRuntime::collect(&model_aliases, &models_cache, routing_settings)
    });
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
        model_router.as_mut(),
        diag,
    );
    (compiled_native_outputs, removed_native_outputs)
}

#[cfg(test)]
mod tests;
