use super::*;
use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
use crate::models::{ModelAlias, ModelSpec};
use indexmap::IndexMap;
use tempfile::TempDir;

fn profile_with_model(model: &str, harness: HarnessKind) -> crate::compiler::agents::AgentProfile {
    crate::compiler::agents::AgentProfile {
        name: None,
        description: None,
        harness: Some(harness),
        model: Some(model.to_string()),
        mode: None,
        model_invocable: true,
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
        default_effort: default_effort.map(str::to_owned),
        autocompact: None,
        autocompact_pct: None,
        spec: ModelSpec::Pinned {
            model: model.to_string(),
            provider: None,
        },
    }
}

#[test]
fn non_cursor_native_model_mapping_handles_pinned_raw_and_unpinned_aliases() {
    let mut aliases = IndexMap::new();
    aliases.insert(
        "sonnet".to_string(),
        pinned_alias_with_harness("claude-sonnet-4-6", "claude", None),
    );
    let mut diag = DiagnosticCollector::new();
    assert_eq!(
        native_model_override_for_harness(
            &HarnessKind::Claude,
            &profile_with_model("sonnet", HarnessKind::Claude),
            &aliases,
            &[],
            &mut diag
        ),
        Some("claude-sonnet-4-6".to_string())
    );
    assert_eq!(
        native_model_override_for_harness(
            &HarnessKind::Codex,
            &profile_with_model("raw-model-id", HarnessKind::Codex),
            &IndexMap::new(),
            &[],
            &mut diag
        ),
        None
    );

    aliases.insert(
        "gpt-auto".to_string(),
        ModelAlias {
            harness: Some("codex".to_string()),
            description: None,
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
    assert_eq!(
        native_model_override_for_harness(
            &HarnessKind::Codex,
            &profile_with_model("gpt-auto", HarnessKind::Codex),
            &aliases,
            &[],
            &mut diag
        ),
        None
    );
    assert!(
        diag.drain()
            .iter()
            .any(|d| d.code == "native-model-alias-unpinned")
    );
}

fn lock_with_target_outputs(targets: &[&str], dest: &str, checksum: &str) -> LockFile {
    let mut lock = LockFile::empty();
    let outputs = targets
        .iter()
        .map(|target| OutputRecord {
            target_root: (*target).to_string(),
            dest_path: dest.into(),
            installed_checksum: checksum.into(),
        })
        .collect();
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
    let removed = reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::SuppressAll,
            project_root: dir.path(),
            model_aliases: &IndexMap::new(),
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: Some(&[HarnessKind::Claude]),
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
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitSelective(spec),
            project_root: dir.path(),
            model_aliases: &aliases,
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &mars_agents,
        &mut diag,
    );

    assert!(
        !dir.path().join(".claude/agents/coder.md").exists(),
        "openai-bound model should not qualify for claude selective reconcile"
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
    let removed = reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitSelective(spec),
            project_root: dir.path(),
            model_aliases: &aliases,
            outcomes: &[],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &mars_agents,
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
