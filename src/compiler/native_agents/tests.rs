use super::*;
use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::host::{
    CapabilityCollectionOptions, CapabilitySession, ExecutableResolver, ExecutableState,
};
use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
use crate::models::{CachedModel, ModelAlias, ModelSpec, ModelsCache};
use indexmap::IndexMap;
use std::path::Path;
use tempfile::TempDir;

fn profile_with_model(model: &str, harness: HarnessKind) -> crate::compiler::agents::AgentProfile {
    crate::compiler::agents::AgentProfile {
        name: None,
        description: None,
        harness: Some(harness),
        model: Some(model.to_string()),
        mode: None,
        model_invocable: true,
        user_invocable: true,
        had_model_invocable_field: false,
        had_user_invocable_field: false,
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

fn pinned_alias_with_harness(
    model: &str,
    harness: &str,
    default_effort: Option<&str>,
) -> ModelAlias {
    ModelAlias {
        harness: Some(harness.to_string()),
        description: None,
        prompting: None,
        default_effort: default_effort.map(str::to_owned),
        autocompact: None,
        autocompact_pct: None,
        spec: ModelSpec::Pinned {
            model: model.to_string(),
            provider: None,
        },
    }
}

fn empty_models_cache() -> ModelsCache {
    ModelsCache {
        models: Vec::new(),
        fetched_at: None,
    }
}

fn models_cache_with(id: &str, provider: &str, release_date: &str) -> ModelsCache {
    ModelsCache {
        models: vec![CachedModel {
            id: id.to_string(),
            provider: provider.to_string(),
            release_date: Some(release_date.to_string()),
            description: None,
            context_window: None,
            max_output: None,
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            cost_reasoning: None,
        }],
        fetched_at: None,
    }
}

struct MissingResolver;

impl ExecutableResolver for MissingResolver {
    fn resolve(&self, _binary: &str) -> ExecutableState {
        ExecutableState::Missing
    }
}

fn test_router<'a>(
    aliases: &'a IndexMap<String, ModelAlias>,
    cache: &'a ModelsCache,
) -> NativeModelRoutingRuntime<'a> {
    let settings = crate::config::Settings::default();
    let routing_settings = ResolvedRoutingSettings::from_settings(&settings);
    let session = CapabilitySession::collect_with_resolver_without_auth(
        &CapabilityCollectionOptions {
            offline: true,
            probe_refresh: crate::models::probes::ProbeRefreshMode::Skip,
        },
        &MissingResolver,
    );
    NativeModelRoutingRuntime::with_session(aliases, cache, routing_settings, session)
}

#[test]
fn native_model_routing_sets_pinned_alias_and_clears_unresolved_candidates() {
    let mut aliases = IndexMap::new();
    aliases.insert(
        "sonnet".to_string(),
        pinned_alias_with_harness("claude-sonnet-4-6", "claude", None),
    );
    aliases.insert(
        "gpt-auto".to_string(),
        ModelAlias {
            harness: Some("codex".to_string()),
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::AutoResolve {
                provider: None,
                match_patterns: vec!["gpt-*".to_string()],
                exclude_patterns: Vec::new(),
            },
        },
    );

    let cache = empty_models_cache();
    let mut router = test_router(&aliases, &cache);
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("sonnet", HarnessKind::Claude),
            &HarnessKind::Claude,
            false,
            true,
        ),
        NativeModelDecision::Set {
            model_id: "claude-sonnet-4-6".to_string()
        }
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("raw-model-id", HarnessKind::Codex),
            &HarnessKind::Codex,
            false,
            true,
        ),
        NativeModelDecision::Clear
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("gpt-auto", HarnessKind::Codex),
            &HarnessKind::Codex,
            false,
            true,
        ),
        NativeModelDecision::Clear
    );
}

#[test]
fn native_model_routing_resolves_auto_aliases_from_models_cache() {
    let mut aliases = IndexMap::new();
    aliases.insert(
        "sonnet".to_string(),
        ModelAlias {
            harness: Some("claude".to_string()),
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::AutoResolve {
                provider: Some("Anthropic".to_string()),
                match_patterns: vec!["*sonnet*".to_string()],
                exclude_patterns: Vec::new(),
            },
        },
    );
    let cache = models_cache_with("claude-sonnet-4-6", "Anthropic", "2026-01-01");
    let mut router = test_router(&aliases, &cache);

    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("sonnet", HarnessKind::Claude),
            &HarnessKind::Cursor,
            false,
            true,
        ),
        NativeModelDecision::Clear,
        "without cursor probe evidence the claude model should not blindly pin to cursor"
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("sonnet", HarnessKind::Claude),
            &HarnessKind::Claude,
            false,
            true,
        ),
        NativeModelDecision::Set {
            model_id: "claude-sonnet-4-6".to_string()
        }
    );
}

#[test]
fn native_model_routing_does_not_linked_fallback_foreign_provider_alias() {
    let mut aliases = IndexMap::new();
    aliases.insert(
        "deepseek".to_string(),
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "deepseek-reasoner".to_string(),
                provider: Some("deepseek".to_string()),
            },
        },
    );
    let cache = empty_models_cache();
    let mut router = test_router(&aliases, &cache);

    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("deepseek", HarnessKind::Claude),
            &HarnessKind::Claude,
            false,
            true,
        ),
        NativeModelDecision::Clear,
        "linked-harness fallback must not make a foreign-provider model qualify for claude"
    );
}

fn lock_with_target_outputs(targets: &[&str], dest: &str, checksum: &str) -> LockFile {
    let mut lock = LockFile::empty();
    let mut outputs: Vec<OutputRecord> = vec![OutputRecord {
        target_root: ".mars".to_string(),
        dest_path: dest.into(),
        installed_checksum: checksum.into(),
    }];
    outputs.extend(targets.iter().map(|target| OutputRecord {
        target_root: (*target).to_string(),
        dest_path: dest.into(),
        installed_checksum: checksum.into(),
    }));
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

#[test]
fn link_suppress_all_reconciles_selective_native_target() {
    let dir = TempDir::new().unwrap();

    let mars_agents_dir = dir.path().join(".mars").join("agents");
    std::fs::create_dir_all(&mars_agents_dir).unwrap();
    std::fs::write(
        mars_agents_dir.join("coder.md"),
        "---\nname: coder\n---\n# Coder\n",
    )
    .unwrap();

    let claude_agents = dir.path().join(".claude").join("agents");
    std::fs::create_dir_all(&claude_agents).unwrap();
    std::fs::write(claude_agents.join("coder.md"), "# Claude native\n").unwrap();

    let codex_agents = dir.path().join(".codex").join("agents");
    std::fs::create_dir_all(&codex_agents).unwrap();
    std::fs::write(codex_agents.join("coder.toml"), "old = true\n").unwrap();

    let mut diag = DiagnosticCollector::new();
    let mut lock =
        lock_with_target_outputs(&[".claude", ".codex"], "agents/coder.md", "sha256:coder");
    lock.items
        .get_mut("agent/coder")
        .unwrap()
        .outputs
        .push(OutputRecord {
            target_root: ".codex".to_string(),
            dest_path: "agents/coder.toml".into(),
            installed_checksum: "sha256:coder-toml".into(),
        });
    let mars_agents = scan_mars_agents(&dir.path().join(".mars"), &mut diag);
    let removed = reconcile_native_agent_surfaces_without_model_routing(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::SuppressAll,
            project_root: dir.path(),
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: Some(&[HarnessKind::Claude]),
            fanout_agents: &[],
        },
        &mars_agents,
        &mut diag,
    );

    assert!(!dir.path().join(".claude/agents/coder.md").exists());
    assert!(
        dir.path().join(".codex/agents/coder.toml").exists(),
        "scoped suppress-all link must not remove unrelated native targets"
    );
    assert!(
        removed.iter().all(|(target, _)| target == ".claude"),
        "removals must stay within the linked harness scope"
    );
    assert!(!removed.is_empty());
}

#[test]
fn reconcile_selective_removes_native_when_agent_stops_qualifying() {
    let dir = TempDir::new().unwrap();
    let mars_agents_dir = dir.path().join(".mars").join("agents");
    std::fs::create_dir_all(&mars_agents_dir).unwrap();
    std::fs::write(
        mars_agents_dir.join("coder.md"),
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
            prompting: None,
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
    let mars_agents = scan_mars_agents(&dir.path().join(".mars"), &mut diag);
    let cache = empty_models_cache();
    let mut router = test_router(&aliases, &cache);
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitSelective(spec),
            project_root: dir.path(),
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
            fanout_agents: &[],
        },
        &mars_agents,
        &mut router,
        &mut diag,
    );

    assert!(
        !dir.path().join(".claude/agents/coder.md").exists(),
        "openai-bound model should not qualify for claude selective reconcile"
    );
}

#[test]
fn emit_selective_fanout_agents_match_case_insensitively() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let agent = parse_mars_agent(
        "---\nname: reviewer\nmodel: gpt-5.3-codex\nmodel-policies:\n  - match:\n      alias: opus\n    override: {}\n---\n# Reviewer\n",
        "reviewer",
    );
    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let spec = agent_copy::AgentCopySpec {
        harnesses: vec![HarnessKind::Claude],
        include_fanout: false,
    };
    let fanout_agents = vec!["Reviewer".to_string()];
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let ctx = NativeAgentCompileCtx {
        project_root: dir.path(),
        old_lock: &LockFile::empty(),
        harness_scope: None,
        configured_emit_harnesses: &[HarnessKind::Claude],
        options: NativeAgentSurfaceCompileOptions {
            force: false,
            collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
            dry_run: false,
        },
        fanout_agents: &fanout_agents,
    };
    let mut router = test_router(&aliases, &models_cache);
    let records = compile_native_agents(
        &ctx,
        &AgentSurfacePolicy::EmitSelective(spec),
        std::slice::from_ref(&agent),
        &mut router,
        &mut diag,
    );

    assert_eq!(records.len(), 1);
    assert!(dir.path().join(".claude/agents/reviewer.md").exists());
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/reviewer.md")).unwrap();
    assert!(
        native.contains("model: claude-opus-4-6"),
        "fanout list entry must qualify reviewer via model-policies despite casing mismatch: {native}"
    );
}

#[test]
#[cfg(unix)]
fn reconcile_selective_keeps_lock_when_native_remove_fails() {
    use std::os::unix::fs::PermissionsExt;

    struct RestoreDirPerms {
        path: std::path::PathBuf,
        mode: u32,
    }

    impl Drop for RestoreDirPerms {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
    }

    let dir = TempDir::new().unwrap();
    let mars_agents_dir = dir.path().join(".mars").join("agents");
    std::fs::create_dir_all(&mars_agents_dir).unwrap();
    std::fs::write(
        mars_agents_dir.join("coder.md"),
        "---\nname: coder\nmodel: opus\n---\n# Coder\n",
    )
    .unwrap();

    let claude_agents = dir.path().join(".claude").join("agents");
    std::fs::create_dir_all(&claude_agents).unwrap();
    let native_path = claude_agents.join("coder.md");
    std::fs::write(&native_path, "# Native\n").unwrap();
    std::fs::set_permissions(&claude_agents, std::fs::Permissions::from_mode(0o555)).unwrap();
    let _restore_agents_dir = RestoreDirPerms {
        path: claude_agents,
        mode: 0o755,
    };

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
            prompting: None,
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
    let mars_agents = scan_mars_agents(&dir.path().join(".mars"), &mut diag);
    let cache = empty_models_cache();
    let mut router = test_router(&aliases, &cache);
    let removed = reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitSelective(spec),
            project_root: dir.path(),
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
            fanout_agents: &[],
        },
        &mars_agents,
        &mut router,
        &mut diag,
    );

    assert!(native_path.exists());
    assert!(
        !removed
            .iter()
            .any(|(target, path)| target == ".claude" && path == "agents/coder.md"),
        "failed delete must not drop lock ownership for .claude/agents/coder.md"
    );
    assert!(
        diag.drain().iter().any(|d| d.code == "native-agent-remove"),
        "failed delete should warn"
    );
}

fn parse_mars_agent(content: &str, stem: &str) -> MarsCanonicalAgent {
    let mut agent_diags = Vec::new();
    let (profile, fm) =
        crate::compiler::agents::parse_agent_content(content, &mut agent_diags).unwrap();
    let agent_name = profile.name.clone().unwrap_or_else(|| stem.to_string());
    MarsCanonicalAgent {
        agent_name,
        canonical_dest_path: format!("agents/{stem}.md"),
        profile,
        fm,
    }
}

fn compile_emit_all_agents(
    dir: &Path,
    configured_harnesses: &[HarnessKind],
    agents: &[MarsCanonicalAgent],
    aliases: &IndexMap<String, ModelAlias>,
) -> Vec<CompiledNativeOutput> {
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let ctx = NativeAgentCompileCtx {
        project_root: dir,
        old_lock: &LockFile::empty(),
        harness_scope: None,
        configured_emit_harnesses: configured_harnesses,
        options: NativeAgentSurfaceCompileOptions {
            force: false,
            collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
            dry_run: false,
        },
        fanout_agents: &[],
    };
    let mut router = test_router(aliases, &models_cache);
    compile_native_agents(
        &ctx,
        &AgentSurfacePolicy::EmitAll,
        agents,
        &mut router,
        &mut diag,
    )
}

#[test]
fn emit_all_emits_every_agent_to_single_configured_claude_target() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let pinned = parse_mars_agent(
        "---\nname: pinned-coder\nmodel: opus\n---\n# Pinned\n",
        "pinned-coder",
    );
    let model_less = parse_mars_agent(
        "---\nname: bare-agent\nmodel: gpt-5.3-codex\n---\n# Bare\n",
        "bare-agent",
    );
    let agents = [pinned, model_less];

    let records = compile_emit_all_agents(dir.path(), &[HarnessKind::Claude], &agents, &aliases);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r.target_root == ".claude"));

    let pinned_native =
        std::fs::read_to_string(dir.path().join(".claude/agents/pinned-coder.md")).unwrap();
    assert!(
        pinned_native.contains("model: claude-opus-4-6"),
        "resolved claude model should be pinned: {pinned_native}"
    );

    let bare_native =
        std::fs::read_to_string(dir.path().join(".claude/agents/bare-agent.md")).unwrap();
    assert!(
        !bare_native.contains("model:"),
        "non-claude model should emit model-less under claude: {bare_native}"
    );
}

#[test]
fn emit_all_emits_to_every_configured_target() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
    std::fs::create_dir_all(dir.path().join(".codex/agents")).unwrap();

    let agent = parse_mars_agent("---\nname: worker\nmodel: opus\n---\n# Worker\n", "worker");
    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "claude-opus-4-6".to_string(),
                provider: Some("anthropic".to_string()),
            },
        },
    );

    let records = compile_emit_all_agents(
        dir.path(),
        &[HarnessKind::Claude, HarnessKind::Codex],
        std::slice::from_ref(&agent),
        &aliases,
    );
    assert_eq!(records.len(), 2);
    assert!(dir.path().join(".claude/agents/worker.md").exists());
    assert!(dir.path().join(".codex/agents/worker.toml").exists());
}

#[test]
fn emit_all_with_empty_configured_targets_emits_nothing() {
    let dir = TempDir::new().unwrap();
    let agent = parse_mars_agent(
        "---\nname: worker\nharness: claude\n---\n# Worker\n",
        "worker",
    );

    let records = compile_emit_all_agents(
        dir.path(),
        &[],
        std::slice::from_ref(&agent),
        &IndexMap::new(),
    );
    assert!(records.is_empty());
    assert!(!dir.path().join(".claude/agents/worker.md").exists());
}

#[test]
fn emit_all_ignores_authored_harness_pin_for_coverage() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let agent = parse_mars_agent(
        "---\nname: product-lead\nharness: codex\nmodel: opus\n---\n# Lead\n",
        "product-lead",
    );
    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "claude-opus-4-6".to_string(),
                provider: Some("anthropic".to_string()),
            },
        },
    );

    let records = compile_emit_all_agents(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
    );
    assert_eq!(records.len(), 1);
    assert!(dir.path().join(".claude/agents/product-lead.md").exists());
    assert!(!dir.path().join(".codex/agents/product-lead.toml").exists());

    let native =
        std::fs::read_to_string(dir.path().join(".claude/agents/product-lead.md")).unwrap();
    assert!(
        native.contains("model: claude-opus-4-6"),
        "authored codex pin should not block claude emission; model resolves via alias: {native}"
    );
}

fn compile_emit_all_with_overlays(
    dir: &Path,
    configured_harnesses: &[HarnessKind],
    agents: &[MarsCanonicalAgent],
    aliases: &IndexMap<String, ModelAlias>,
    overlays: &IndexMap<String, crate::config::AgentOverlay>,
) -> Vec<CompiledNativeOutput> {
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let ctx = NativeAgentCompileCtx {
        project_root: dir,
        old_lock: &LockFile::empty(),
        harness_scope: None,
        configured_emit_harnesses: configured_harnesses,
        options: NativeAgentSurfaceCompileOptions {
            force: false,
            collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
            dry_run: false,
        },
        fanout_agents: &[],
    };
    // Mirror the lifecycle: resolve overlays before compile (compile no longer merges).
    let resolved = resolve_native_agent_profiles(agents, overlays);
    let mut router = test_router(aliases, &models_cache);
    compile_native_agents(
        &ctx,
        &AgentSurfacePolicy::EmitAll,
        &resolved,
        &mut router,
        &mut diag,
    )
}

#[test]
fn emit_all_consumes_overlay_model_over_profile_model() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    // profile.model is a codex token that does not resolve to claude.
    let agent = parse_mars_agent(
        "---\nname: worker\nmodel: gpt-5.3-codex\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            model: Some("opus".to_string()),
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &overlays,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        native.contains("model: claude-opus-4-6"),
        "overlay.model must re-pin the claude native copy: {native}"
    );
}

#[test]
fn emit_all_consumes_overlay_model_policies() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    // profile has no claude-resolving model; an overlay model-policy supplies one.
    let agent = parse_mars_agent(
        "---\nname: worker\nmodel: gpt-5.3-codex\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            model_policies: vec![crate::config::ModelPolicyRule {
                match_type: crate::config::ModelPolicyMatchType::Alias,
                match_value: "opus".to_string(),
                no_fallback: false,
                overrides: serde_yaml::Mapping::new(),
            }],
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &overlays,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        native.contains("model: claude-opus-4-6"),
        "overlay model-policy must supply the claude native model: {native}"
    );
}

#[test]
fn emit_all_overlay_cross_harness_clears_foreign_declared_harness() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
    std::fs::create_dir_all(dir.path().join(".codex/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );
    aliases.insert(
        "codex-model".to_string(),
        pinned_alias_with_harness("gpt-5.3-codex", "codex", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\nharness: codex\nmodel: codex-model\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            model: Some("opus".to_string()),
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude, HarnessKind::Codex],
        std::slice::from_ref(&agent),
        &aliases,
        &overlays,
    );
    assert_eq!(records.len(), 2);
    assert!(dir.path().join(".claude/agents/worker.md").exists());
    assert!(dir.path().join(".codex/agents/worker.toml").exists());

    let codex_native =
        std::fs::read_to_string(dir.path().join(".codex/agents/worker.toml")).unwrap();
    assert!(
        !codex_native.contains("model"),
        "overlay claude model must not leak into declared codex harness: {codex_native}"
    );

    let claude_native =
        std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        claude_native.contains("model: claude-opus-4-6"),
        "claude harness should carry the overlay opus model: {claude_native}"
    );
}

#[test]
fn emit_all_hand_authored_cross_harness_clears_foreign_model() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "codex-model".to_string(),
        pinned_alias_with_harness("gpt-5.3-codex", "codex", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\nharness: claude\nmodel: codex-model\n---\n# Worker\n",
        "worker",
    );

    let records = compile_emit_all_agents(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        !native.contains("model:"),
        "declared claude harness with codex model must emit model-less: {native}"
    );
}

#[test]
fn emit_all_declared_harness_matching_model_still_pins() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let pinned = parse_mars_agent(
        "---\nname: pinned-worker\nharness: claude\nmodel: opus\n---\n# Pinned\n",
        "pinned-worker",
    );
    let model_less = parse_mars_agent(
        "---\nname: bare-worker\nharness: claude\n---\n# Bare\n",
        "bare-worker",
    );

    let records = compile_emit_all_agents(
        dir.path(),
        &[HarnessKind::Claude],
        &[pinned, model_less],
        &aliases,
    );
    assert_eq!(records.len(), 2);

    let pinned_native =
        std::fs::read_to_string(dir.path().join(".claude/agents/pinned-worker.md")).unwrap();
    assert!(
        pinned_native.contains("model: claude-opus-4-6"),
        "matching harness+model should still pin: {pinned_native}"
    );

    let bare_native =
        std::fs::read_to_string(dir.path().join(".claude/agents/bare-worker.md")).unwrap();
    assert!(
        !bare_native.contains("model:"),
        "declared harness without model should emit model-less: {bare_native}"
    );
}

#[test]
fn emit_all_ignores_overlay_harness_for_model_resolution() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let aliases = IndexMap::new();
    // profile.model is a codex token; overlay.harness alone must not make it qualify.
    let agent = parse_mars_agent(
        "---\nname: worker\nmodel: gpt-5.3-codex\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            harness: Some("claude".to_string()),
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &overlays,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        !native.contains("model:"),
        "overlay.harness must be ignored; the codex model must not leak under claude: {native}"
    );
}

#[test]
fn emit_all_empty_overlay_leaves_lowered_output_unchanged() {
    let dir_baseline = TempDir::new().unwrap();
    std::fs::create_dir_all(dir_baseline.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\ndescription: Profile desc\nmodel: opus\ntools: [Bash]\n---\n# Worker\n",
        "worker",
    );
    compile_emit_all_with_overlays(
        dir_baseline.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &IndexMap::new(),
    );
    let baseline = std::fs::read_to_string(
        dir_baseline
            .path()
            .join(".claude/agents/worker.md"),
    )
    .unwrap();

    let dir_empty = TempDir::new().unwrap();
    std::fs::create_dir_all(dir_empty.path().join(".claude/agents")).unwrap();
    compile_emit_all_with_overlays(
        dir_empty.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &IndexMap::from([(
            "worker".to_string(),
            crate::config::AgentOverlay::default(),
        )]),
    );
    let empty_overlay_native = std::fs::read_to_string(
        dir_empty.path().join(".claude/agents/worker.md"),
    )
    .unwrap();
    assert_eq!(
        baseline, empty_overlay_native,
        "default/empty overlay must not change lowered native output"
    );
}

#[test]
fn emit_all_consumes_overlay_description_and_tools_disallowed() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\ndescription: Profile desc\nmodel: opus\ntools: [Bash]\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            description: Some("Overlay description".to_string()),
            tools: crate::config::AgentOverlayTools {
                disallowed: vec!["Write".to_string()],
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &overlays,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        native.contains("description: Overlay description"),
        "overlay.description must replace profile description: {native}"
    );
    assert!(
        native.contains("disallowed-tools:") && native.contains("Write"),
        "overlay tools.disallowed must appear in lowered output: {native}"
    );
    assert!(
        !native.contains("Profile desc"),
        "profile description must be replaced: {native}"
    );
}

#[test]
fn emit_all_consumes_overlay_user_invocable_on_effective_profile() {
    let agent = parse_mars_agent(
        "---\nname: worker\nmodel: opus\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            user_invocable: Some(false),
            ..Default::default()
        },
    );

    let resolved = resolve_native_agent_profiles(std::slice::from_ref(&agent), &overlays);
    let profile = &resolved[0].profile;
    assert!(!profile.user_invocable);
    assert!(
        profile.had_user_invocable_field,
        "overlay user_invocable must set presence bit for lowering"
    );
}

#[test]
fn emit_all_merged_local_overlay_wins_for_description() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\ndescription: Profile desc\nmodel: opus\n---\n# Worker\n",
        "worker",
    );

    let mut base_agents = IndexMap::new();
    base_agents.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            description: Some("mars.toml description".to_string()),
            ..Default::default()
        },
    );
    let mut local_agents = IndexMap::new();
    local_agents.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            description: Some("mars.local.toml description".to_string()),
            ..Default::default()
        },
    );
    let merged = crate::config::merged_agent_overlays(
        &base_agents,
        &crate::config::LocalConfig {
            agents: local_agents,
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessKind::Claude],
        std::slice::from_ref(&agent),
        &aliases,
        &merged,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".claude/agents/worker.md")).unwrap();
    assert!(
        native.contains("description: mars.local.toml description"),
        "mars.local.toml overlay must replace mars.toml for widened fields: {native}"
    );
}
