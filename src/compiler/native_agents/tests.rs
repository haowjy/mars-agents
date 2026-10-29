use super::*;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::host::{
    CapabilityCollectionOptions, CapabilitySession, ExecutableResolver, ExecutableState,
};
use crate::lock::{LockFile, LockedItemV2, OutputRecord};
use crate::models::{CachedModel, ModelAlias, ModelSpec, ModelsCache};
use indexmap::IndexMap;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;
use std::path::Path;
use tempfile::TempDir;

fn profile_with_model(model: &str, harness: HarnessId) -> mars_schema::agent::AgentProfile {
    mars_schema::agent::AgentProfile {
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
        network: None,
        network_allow: Default::default(),
        network_deny: Default::default(),
        effort: None,
        skills: mars_schema::frontmatter::SkillsSpec::default(),
        subagents: Vec::new(),
        tools: Default::default(),
        harness_overrides: mars_schema::harness_overrides::HarnessOverrides::default(),
        model_policies: Vec::new(),
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
    let session = CapabilitySession::collect_with_resolver(
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
            &profile_with_model("sonnet", HarnessId::Claude),
            None,
            &HarnessId::Claude,
            false,
            true,
        ),
        NativeModelDecision::Set {
            model_id: "claude-sonnet-4-6".to_string(),
            token: "sonnet".to_string(),
        }
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("raw-model-id", HarnessId::Codex),
            None,
            &HarnessId::Codex,
            false,
            true,
        ),
        NativeModelDecision::Clear
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("gpt-auto", HarnessId::Codex),
            None,
            &HarnessId::Codex,
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
            &profile_with_model("sonnet", HarnessId::Claude),
            None,
            &HarnessId::Cursor,
            false,
            true,
        ),
        NativeModelDecision::Clear,
        "without cursor probe evidence the claude model should not blindly pin to cursor"
    );
    assert_eq!(
        router.decision_for_profile(
            &profile_with_model("sonnet", HarnessId::Claude),
            None,
            &HarnessId::Claude,
            false,
            true,
        ),
        NativeModelDecision::Set {
            model_id: "claude-sonnet-4-6".to_string(),
            token: "sonnet".to_string(),
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
            &profile_with_model("deepseek", HarnessId::Claude),
            None,
            &HarnessId::Claude,
            false,
            true,
        ),
        NativeModelDecision::Clear,
        "linked-harness fallback must not make a foreign-provider model qualify for claude"
    );
}

fn lock_with_target_outputs(targets: &[&str], dest: &str, checksum: &str) -> LockFile {
    let mut lock = LockFile::empty();
    let mut outputs: Vec<OutputRecord> = vec![OutputRecord::installed(
        ".mars".to_string(),
        dest.into(),
        checksum.into(),
    )];
    outputs.extend(targets.iter().map(|target| {
        OutputRecord::installed((*target).to_string(), dest.into(), checksum.into())
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
        .push(OutputRecord::installed(
            ".codex".to_string(),
            "agents/coder.toml".into(),
            "sha256:coder-toml".into(),
        ));
    let removed = reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            project_root: dir.path(),
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: Some(&[HarnessId::Claude]),
        },
        &HashSet::new(),
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
fn reconcile_removes_native_claim_not_in_desired_set() {
    let dir = TempDir::new().unwrap();
    let claude_agents = dir.path().join(".claude").join("agents");
    std::fs::create_dir_all(&claude_agents).unwrap();
    std::fs::write(claude_agents.join("coder.md"), "# Native\n").unwrap();

    let lock = lock_with_target_outputs(&[".claude"], "agents/coder.md", "sha256:coder");
    let mut diag = DiagnosticCollector::new();
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            project_root: dir.path(),
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &HashSet::new(),
        &mut diag,
    );

    assert!(
        !dir.path().join(".claude/agents/coder.md").exists(),
        "a prior native claim outside this run's exact emission set must be removed"
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
        harnesses: vec![HarnessId::Claude],
        include_fanout: false,
    };
    let fanout_agents = vec!["Reviewer".to_string()];
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let empty_lock = LockFile::empty();
    let ctx = native_ctx(
        dir.path(),
        &empty_lock,
        &[HarnessId::Claude],
        &fanout_agents,
    );
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
    let claude_agents = dir.path().join(".claude").join("agents");
    std::fs::create_dir_all(&claude_agents).unwrap();
    let native_path = claude_agents.join("coder.md");
    std::fs::write(&native_path, "# Native\n").unwrap();
    std::fs::set_permissions(&claude_agents, std::fs::Permissions::from_mode(0o555)).unwrap();
    let _restore_agents_dir = RestoreDirPerms {
        path: claude_agents.clone(),
        mode: 0o755,
    };
    // Root ignores mode bits, so the removal would not fail; there is nothing to test.
    if std::fs::write(claude_agents.join("probe"), "").is_ok() {
        return;
    }

    let lock = lock_with_target_outputs(&[".claude"], "agents/coder.md", "sha256:coder");
    let mut diag = DiagnosticCollector::new();
    let removed = reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            project_root: dir.path(),
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &HashSet::new(),
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
    let reading = mars_schema::agent::read(content);
    let (profile, body) = (reading.fields.unwrap(), reading.body.unwrap());
    let agent_name = profile.name.clone().unwrap_or_else(|| stem.to_string());
    MarsCanonicalAgent {
        agent_name,
        canonical_dest_path: format!("agents/{stem}.md"),
        profile,
        overlay: None,
        body,
    }
}

fn native_ctx<'a>(
    project_root: &'a Path,
    old_lock: &'a LockFile,
    configured_emit_harnesses: &'a [HarnessId],
    fanout_agents: &'a [String],
) -> NativeAgentCompileCtx<'a> {
    NativeAgentCompileCtx {
        project_root,
        old_lock,
        harness_scope: None,
        configured_emit_harnesses,
        options: NativeAgentSurfaceCompileOptions {
            force: false,
            collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
            dry_run: false,
        },
        fanout_agents,
    }
}

fn compile_emit_all_agents(
    dir: &Path,
    configured_harnesses: &[HarnessId],
    agents: &[MarsCanonicalAgent],
    aliases: &IndexMap<String, ModelAlias>,
) -> Vec<CompiledNativeOutput> {
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let empty_lock = LockFile::empty();
    let ctx = native_ctx(dir, &empty_lock, configured_harnesses, &[]);
    let mut router = test_router(aliases, &models_cache);
    compile_native_agents(
        &ctx,
        &AgentSurfacePolicy::EmitAll,
        agents,
        &mut router,
        &mut diag,
    )
}

fn compile_emit_all_opencode(
    dir: &Path,
    agent: &MarsCanonicalAgent,
    aliases: &IndexMap<String, ModelAlias>,
    probe: Option<crate::models::probes::OpenCodeProbeResult>,
    provider_order: Option<Vec<String>>,
) -> Vec<CompiledNativeOutput> {
    let mut diag = DiagnosticCollector::new();
    let cache = empty_models_cache();
    let settings = crate::config::Settings {
        provider_order,
        ..Default::default()
    };
    let routing = ResolvedRoutingSettings::from_settings(&settings);
    let session = CapabilitySession::collect_with_resolver(
        &CapabilityCollectionOptions {
            offline: true,
            probe_refresh: crate::models::probes::ProbeRefreshMode::Skip,
        },
        &MissingResolver,
    );
    let mut router = NativeModelRoutingRuntime::with_session(aliases, &cache, routing, session);
    if let Some(probe) = probe {
        router.session.set_opencode_probe_for_test(probe);
    }
    let empty_lock = LockFile::empty();
    let ctx = native_ctx(dir, &empty_lock, &[HarnessId::OpenCode], &[]);
    compile_native_agents(
        &ctx,
        &AgentSurfacePolicy::EmitAll,
        std::slice::from_ref(agent),
        &mut router,
        &mut diag,
    )
}

#[test]
fn opencode_native_emission_qualifies_alias_and_honors_provider_preference() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".opencode/agents")).unwrap();
    let mut aliases = IndexMap::new();
    aliases.insert(
        "gpt".to_string(),
        ModelAlias {
            harness: Some("opencode".to_string()),
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: ModelSpec::Pinned {
                model: "gpt-5.4".into(),
                provider: None,
            },
        },
    );
    let agent = parse_mars_agent("---\nname: worker\nmodel: gpt\n---\n# Worker\n", "worker");
    let probe = crate::models::probes::OpenCodeProbeResult {
        model_slugs: vec!["openai/gpt-5.4".into(), "openrouter/gpt-5.4".into()],
        model_probe_success: true,
        error: None,
    };
    let records = compile_emit_all_opencode(
        dir.path(),
        &agent,
        &aliases,
        Some(probe),
        Some(vec!["openrouter".into()]),
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".opencode/agents/worker.md")).unwrap();
    assert!(native.contains("model: openrouter/gpt-5.4"), "{native}");
}

#[test]
fn opencode_native_emission_preserves_qualified_input_and_skips_unavailable_bare_route() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".opencode/agents")).unwrap();
    let qualified = parse_mars_agent(
        "---\nname: qualified\nmodel: openai/gpt-5.4\n---\n# Qualified\n",
        "qualified",
    );
    let unavailable = parse_mars_agent(
        "---\nname: unavailable\nmodel: unknown-model\n---\n# Unknown\n",
        "unavailable",
    );
    let aliases = IndexMap::new();
    let qualified_probe = crate::models::probes::OpenCodeProbeResult {
        // The explicit provider must win over the broad provider-variant
        // preference used for aliases.
        model_slugs: vec!["openai/gpt-5.4".into(), "openai-codex/gpt-5.4".into()],
        model_probe_success: true,
        error: None,
    };
    let records = compile_emit_all_opencode(
        dir.path(),
        &qualified,
        &aliases,
        Some(qualified_probe),
        None,
    );
    assert_eq!(records.len(), 1);
    let native = std::fs::read_to_string(dir.path().join(".opencode/agents/qualified.md")).unwrap();
    assert!(native.contains("model: openai/gpt-5.4"), "{native}");
    let unavailable_records =
        compile_emit_all_opencode(dir.path(), &unavailable, &aliases, None, None);
    assert_eq!(unavailable_records.len(), 1);
    let unavailable_native =
        std::fs::read_to_string(dir.path().join(".opencode/agents/unavailable.md")).unwrap();
    assert!(
        !unavailable_native.contains("model: unknown-model"),
        "{unavailable_native}"
    );
}

#[test]
fn emit_all_pins_a_model_only_when_it_resolves_to_the_target_harness() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );
    aliases.insert(
        "codex-model".to_string(),
        pinned_alias_with_harness("gpt-5.3-codex", "codex", None),
    );

    // (agent source, name, expected pinned model on the claude copy)
    let cases = [
        // No declared harness: the model decides.
        ("model: opus", "pinned-coder", Some("claude-opus-4-6")),
        ("model: gpt-5.3-codex", "bare-agent", None),
        // An authored harness pin does not block coverage of other targets.
        (
            "harness: codex\nmodel: opus",
            "product-lead",
            Some("claude-opus-4-6"),
        ),
        // Declared claude harness: a foreign model is cleared, a matching one pins.
        (
            "harness: claude\nmodel: codex-model",
            "foreign-worker",
            None,
        ),
        (
            "harness: claude\nmodel: opus",
            "pinned-worker",
            Some("claude-opus-4-6"),
        ),
        ("harness: claude", "bare-worker", None),
    ];
    let agents: Vec<_> = cases
        .iter()
        .map(|(front, name, _)| {
            parse_mars_agent(&format!("---\nname: {name}\n{front}\n---\n# Body\n"), name)
        })
        .collect();

    let records = compile_emit_all_agents(dir.path(), &[HarnessId::Claude], &agents, &aliases);
    assert_eq!(records.len(), cases.len());
    assert!(records.iter().all(|r| r.target_root == ".claude"));
    assert!(!dir.path().join(".codex").exists());

    for (_, name, expected) in cases {
        let native =
            std::fs::read_to_string(dir.path().join(format!(".claude/agents/{name}.md"))).unwrap();
        match expected {
            Some(model) => assert!(
                native.contains(&format!("model: {model}")),
                "{name} should pin {model}: {native}"
            ),
            None => assert!(
                !native.contains("model:"),
                "{name} should emit model-less: {native}"
            ),
        }
    }
}

fn compile_emit_all_with_overlays(
    dir: &Path,
    configured_harnesses: &[HarnessId],
    agents: &[MarsCanonicalAgent],
    aliases: &IndexMap<String, ModelAlias>,
    overlays: &IndexMap<String, crate::config::AgentOverlay>,
) -> Vec<CompiledNativeOutput> {
    let mut diag = DiagnosticCollector::new();
    let models_cache = empty_models_cache();
    let empty_lock = LockFile::empty();
    let ctx = native_ctx(dir, &empty_lock, configured_harnesses, &[]);
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
fn pending_deletion_does_not_authorize_native_agent_overwrite() {
    let dir = TempDir::new().unwrap();
    let native_path = dir.path().join(".claude/agents/coder.md");
    std::fs::create_dir_all(native_path.parent().unwrap()).unwrap();
    std::fs::write(&native_path, "# User replacement\n").unwrap();

    let agent = parse_mars_agent("---\nname: coder\n---\n# Mars agent\n", "coder");
    let mut lock = lock_with_target_outputs(&[".claude"], "agents/coder.md", "sha256:old");
    lock.items
        .get_mut("agent/coder")
        .unwrap()
        .outputs
        .iter_mut()
        .find(|output| output.target_root == ".claude")
        .unwrap()
        .mark_pending_deletion();

    let models_cache = empty_models_cache();
    let aliases = IndexMap::new();
    let mut router = test_router(&aliases, &models_cache);
    let mut diag = DiagnosticCollector::new();
    let records = compile_native_agents(
        &native_ctx(dir.path(), &lock, &[HarnessId::Claude], &[]),
        &AgentSurfacePolicy::EmitAll,
        &[agent],
        &mut router,
        &mut diag,
    );

    assert!(records.is_empty());
    assert_eq!(
        std::fs::read_to_string(native_path).unwrap(),
        "# User replacement\n"
    );
    let diagnostics = diag.drain();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "target-unmanaged-collision")
    );
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "native-agent-projection-repaired"),
        "a tombstone has no installed claim to diverge from"
    );
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
        &[HarnessId::Claude],
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
            model_policies: Some(vec![mars_schema::model_policy::ModelPolicyRule {
                match_type: mars_schema::model_policy::ModelPolicyMatchType::Alias,
                match_value: "opus".to_string(),
                no_fallback: false,
                overrides: Default::default(),
                when: None,
            }]),
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessId::Claude],
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
        &[HarnessId::Claude],
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
fn emit_all_consumes_overlay_description_and_tools_disallowed() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();

    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias_with_harness("claude-opus-4-6", "claude", None),
    );

    let agent = parse_mars_agent(
        "---\nname: worker\ndescription: Profile desc\nmodel: opus\ntools:\n  '*': deny\n  Bash: allow\n---\n# Worker\n",
        "worker",
    );
    let mut overlays: IndexMap<String, crate::config::AgentOverlay> = IndexMap::new();
    overlays.insert(
        "worker".to_string(),
        crate::config::AgentOverlay {
            description: Some("Overlay description".to_string()),
            tools: crate::config::AgentOverlayTools {
                disallowed: Some(vec!["Write".to_string().try_into().unwrap()].into()),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let records = compile_emit_all_with_overlays(
        dir.path(),
        &[HarnessId::Claude],
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
