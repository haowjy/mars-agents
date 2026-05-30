use super::*;
use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::lock::{ItemId, ItemKind, LockFile, LockedItemV2, OutputRecord};
use crate::models::{ModelAlias, ModelSpec};
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::{DestPath, ItemName};
use indexmap::IndexMap;
use tempfile::TempDir;

#[test]
fn selective_orphan_preserve_paths_include_native_lock_records() {
    let spec = agent_copy::AgentCopySpec {
        harnesses: vec![HarnessKind::Claude],
        include_fanout: false,
    };
    let lock = lock_with_target_outputs(&[".claude"], "agents/coder.md", "sha256:coder");
    let preserved = selective_native_orphan_preserve_paths(&lock, &spec);
    assert!(
        preserved
            .get(".claude")
            .is_some_and(|paths| paths.contains("agents/coder.md"))
    );
}

fn profile_with_cursor_model(model: &str) -> crate::compiler::agents::AgentProfile {
    crate::compiler::agents::AgentProfile {
        name: None,
        description: None,
        harness: Some(HarnessKind::Cursor),
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

fn pinned_alias(model: &str, default_effort: Option<&str>) -> ModelAlias {
    ModelAlias {
        harness: Some("codex".to_string()),
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
fn cursor_native_model_mapping_uses_shared_resolver_for_alias_and_effort() {
    let profile = profile_with_cursor_model("gpt55");
    let mut aliases = IndexMap::new();
    aliases.insert("gpt55".to_string(), pinned_alias("gpt-5.5", Some("high")));
    let slugs = vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()];
    assert_eq!(
        native_model_override_for_harness(&HarnessKind::Cursor, &profile, &aliases, &slugs),
        Some("gpt-5.5-high".to_string())
    );
}

#[test]
fn cursor_native_model_mapping_preserves_unknown_or_cursor_literal_tokens() {
    let profile = profile_with_cursor_model("composer-2.5[fast=false]");
    let slugs = vec!["composer-2.5".to_string(), "composer-2.5-fast".to_string()];
    assert_eq!(
        native_model_override_for_harness(&HarnessKind::Cursor, &profile, &IndexMap::new(), &slugs),
        None
    );

    let profile = profile_with_cursor_model("unmapped-model");
    assert_eq!(
        native_model_override_for_harness(&HarnessKind::Cursor, &profile, &IndexMap::new(), &slugs),
        None
    );
}

#[test]
fn cursor_native_model_mapping_uses_claude_shim_with_shared_resolver() {
    let profile = profile_with_cursor_model("opus");
    let mut aliases = IndexMap::new();
    aliases.insert(
        "opus".to_string(),
        pinned_alias("claude-opus-4-6", Some("high")),
    );
    let slugs = vec![
        "claude-4.6-opus-thinking-high".to_string(),
        "claude-4.6-opus-thinking-medium".to_string(),
    ];

    assert_eq!(
        native_model_override_for_harness(&HarnessKind::Cursor, &profile, &aliases, &slugs),
        Some("claude-4.6-opus-thinking-high".to_string())
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

fn agent_outcome(name: &str, action: ActionTaken) -> ActionOutcome {
    ActionOutcome {
        item_id: ItemId {
            kind: ItemKind::Agent,
            name: ItemName::from(name),
        },
        action,
        dest_path: DestPath::from(format!("agents/{name}.md")),
        source_name: "test-source".into(),
        source_checksum: None,
        installed_checksum: None,
    }
}

#[test]
fn reconcile_emit_all_removes_native_shapes_for_removed_agents() {
    let dir = TempDir::new().unwrap();
    for harness in HarnessKind::all() {
        let agents_dir = dir.path().join(harness.target_dir()).join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("coder.md"), "# Old\n").unwrap();
        std::fs::write(agents_dir.join("coder.toml"), "old = true\n").unwrap();
    }

    let tracked_targets: Vec<&str> = HarnessKind::all().iter().map(|h| h.target_dir()).collect();
    let mut lock = lock_with_target_outputs(&tracked_targets, "agents/coder.md", "sha256:coder");
    for target in &tracked_targets {
        lock.items
            .get_mut("agent/coder")
            .unwrap()
            .outputs
            .push(OutputRecord {
                target_root: (*target).to_string(),
                dest_path: "agents/coder.toml".into(),
                installed_checksum: "sha256:coder-toml".into(),
            });
    }

    let mut diag = DiagnosticCollector::new();
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitAll,
            project_root: dir.path(),
            model_aliases: &IndexMap::new(),
            outcomes: &[agent_outcome("coder", ActionTaken::Removed)],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &[],
        &mut diag,
    );

    for harness in HarnessKind::all() {
        assert!(
            !dir.path()
                .join(harness.target_dir())
                .join("agents/coder.md")
                .exists()
        );
        assert!(
            !dir.path()
                .join(harness.target_dir())
                .join("agents/coder.toml")
                .exists()
        );
    }
    assert!(diag.drain().is_empty());
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
fn reconcile_suppress_all_removes_native_shapes_for_current_agents() {
    let dir = TempDir::new().unwrap();

    let mars_agents_dir = dir.path().join(".mars").join("agents");
    std::fs::create_dir_all(&mars_agents_dir).unwrap();
    std::fs::write(
        mars_agents_dir.join("coder.md"),
        "---\nname: coder\n---\n# Coder\n",
    )
    .unwrap();

    for target in [".claude", ".codex", ".opencode"] {
        let agents_dir = dir.path().join(target).join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("coder.md"), "# Native\n").unwrap();
    }

    let mut diag = DiagnosticCollector::new();
    let lock = lock_with_target_outputs(
        &[".claude", ".codex", ".opencode"],
        "agents/coder.md",
        "sha256:coder",
    );
    let mars_agents = scan_mars_agents(&dir.path().join(".mars"), &mut diag);
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::SuppressAll,
            project_root: dir.path(),
            model_aliases: &IndexMap::new(),
            outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
            old_lock: &lock,
            dry_run: false,
            selective_harness_scope: None,
        },
        &mars_agents,
        &mut diag,
    );

    for target in [".claude", ".codex", ".opencode"] {
        assert!(
            !dir.path().join(target).join("agents/coder.md").exists(),
            "native agent should be removed under SuppressAll for target {target}"
        );
    }
}

#[test]
fn reconcile_suppress_all_preserves_untracked_native_agents() {
    let dir = TempDir::new().unwrap();

    let mars_agents_dir = dir.path().join(".mars").join("agents");
    std::fs::create_dir_all(&mars_agents_dir).unwrap();
    std::fs::write(
        mars_agents_dir.join("coder.md"),
        "---\nname: coder\n---\n# Coder\n",
    )
    .unwrap();

    let agents_dir = dir.path().join(".cursor").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("coder.md"), "# hand-written\n").unwrap();

    let mut diag = DiagnosticCollector::new();
    let mars_agents = scan_mars_agents(&dir.path().join(".mars"), &mut diag);
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::SuppressAll,
            project_root: dir.path(),
            model_aliases: &IndexMap::new(),
            outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
            old_lock: &LockFile::empty(),
            dry_run: false,
            selective_harness_scope: None,
        },
        &mars_agents,
        &mut diag,
    );

    assert!(dir.path().join(".cursor/agents/coder.md").exists());
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

#[test]
fn reconcile_emit_all_preserves_non_removed_agents() {
    let dir = TempDir::new().unwrap();

    let agents_dir = dir.path().join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("coder.md"), "# Native\n").unwrap();

    let mut diag = DiagnosticCollector::new();
    reconcile_native_agent_surfaces(
        &NativeAgentReconcileCtx {
            policy: AgentSurfacePolicy::EmitAll,
            project_root: dir.path(),
            model_aliases: &IndexMap::new(),
            outcomes: &[agent_outcome("coder", ActionTaken::Installed)],
            old_lock: &LockFile::empty(),
            dry_run: false,
            selective_harness_scope: None,
        },
        &[],
        &mut diag,
    );

    assert!(dir.path().join(".claude/agents/coder.md").exists());
}
