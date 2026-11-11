use super::*;
use crate::diagnostic::DiagnosticCollector;
use crate::harness::host::{
    CapabilityCollectionOptions, CapabilitySession, ExecutableResolver, ExecutableState,
};
use crate::lock::{LockFile, LockedItemV2, OutputRecord};
use crate::models::{ModelAlias, ModelsCache};
use indexmap::IndexMap;
use mars_schema::HarnessId;
use mars_schema::item::ItemKind;
use tempfile::TempDir;

fn empty_models_cache() -> ModelsCache {
    ModelsCache {
        models: Vec::new(),
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
            source_path: None,
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

fn parse_mars_agent(content: &str, stem: &str) -> MarsCanonicalAgent {
    let reading = mars_schema::agent::read(content);
    let (profile, body) = (reading.fields.unwrap(), reading.body.unwrap());
    let agent_name = profile.name.clone().unwrap_or_else(|| stem.to_string());
    MarsCanonicalAgent {
        agent_name,
        canonical_dest_path: format!("agents/{stem}.md"),
        profile,
        overlay: None,
        settlement: Default::default(),
        body,
    }
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
        &NativeAgentCompileCtx {
            project_root: dir.path(),
            old_lock: &lock,
            harness_scope: None,
            configured_emit_harnesses: &[HarnessId::Claude],
            options: NativeAgentSurfaceCompileOptions {
                force: false,
                collision_hint: crate::surface_ownership::CollisionAdoptHint::SyncForce,
                dry_run: false,
                preview_warnings: false,
            },
            fanout_agents: &[],
        },
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

fn compile_emit_all_with_overlays(
    dir: &Path,
    configured_harnesses: &[HarnessId],
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
            preview_warnings: false,
        },
        fanout_agents: &[],
    };
    // Mirror the lifecycle: resolve overlays before compile (compile no longer merges).
    let resolved = resolve_native_agent_profiles(
        agents,
        ToolConfig {
            agent_overlays: overlays,
            project_tools: &ToolRuleMap::default(),
        },
    );
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
