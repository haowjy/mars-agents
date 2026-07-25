use super::support::*;

#[test]
fn hook_identity_and_materialization_are_scoped_per_target() {
    let dir = TempDir::new().unwrap();
    let claude_source = dir.child("claude-source");
    let codex_source = dir.child("codex-source");
    claude_source.create_dir_all().unwrap();
    codex_source.create_dir_all().unwrap();
    write_dependency_hook(
        &claude_source,
        "audit",
        ".claude",
        "SessionStart",
        "printf dep-claude",
    );
    write_dependency_hook(
        &codex_source,
        "audit",
        ".codex",
        "SessionStart",
        "printf dep-codex",
    );

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\", \".codex\"]\n[dependencies]\nclaude-source = {{ path = \"{}\" }}\ncodex-source = {{ path = \"{}\" }}\n",
            portable_path(claude_source.path()),
            portable_path(codex_source.path())
        ))
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf local-claude"}]}]}"#,
    );

    sync(&project).success();
    let claude = fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap();
    let codex = fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap();
    assert!(claude.contains("local-claude"));
    assert!(!claude.contains("dep-claude"));
    assert!(codex.contains("dep-codex"));
    project
        .child(".claude/hooks/audit/claude.json")
        .assert(predicate::str::contains("local-claude"));
    project
        .child(".codex/hooks/audit/codex.json")
        .assert(predicate::str::contains("dep-codex"));
}

#[test]
fn same_name_hooks_keep_target_scoped_ownership_when_removed_independently() {
    let dir = TempDir::new().unwrap();
    let claude_source = dir.child("claude-source");
    let codex_source = dir.child("codex-source");
    claude_source.create_dir_all().unwrap();
    codex_source.create_dir_all().unwrap();
    write_dependency_hook(
        &claude_source,
        "audit",
        ".claude",
        "SessionStart",
        "printf claude",
    );
    write_dependency_hook(
        &codex_source,
        "audit",
        ".codex",
        "SessionStart",
        "printf codex",
    );
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    let config = project.child("mars.toml");
    let write_config = |claude: bool, codex: bool| {
        let mut raw = String::from("[settings]\ntargets = [\".claude\", \".codex\"]\n");
        if claude || codex {
            raw.push_str("[dependencies]\n");
        }
        if claude {
            raw.push_str(&format!(
                "claude-source = {{ path = \"{}\" }}\n",
                portable_path(claude_source.path())
            ));
        }
        if codex {
            raw.push_str(&format!(
                "codex-source = {{ path = \"{}\" }}\n",
                portable_path(codex_source.path())
            ));
        }
        config.write_str(&raw).unwrap();
    };

    write_config(true, true);
    sync(&project).success();
    assert_hook_target_owner(&project, "hooks/claude/audit", ".claude");
    assert_hook_target_owner(&project, "hooks/codex/audit", ".codex");

    write_config(false, true);
    sync(&project).success();
    project
        .child(".claude/hooks/audit")
        .assert(predicate::path::missing());
    project
        .child(".codex/hooks/audit/run.sh")
        .assert("#!/bin/sh\n");
    assert_hook_target_owner(&project, "hooks/codex/audit", ".codex");

    write_config(false, false);
    sync(&project).success();
    project
        .child(".codex/hooks/audit")
        .assert(predicate::path::missing());
}

#[test]
fn unmanaged_target_hook_directory_fails_before_emission_or_canonical_mutation() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    project
        .child(".claude/hooks/audit")
        .create_dir_all()
        .unwrap();
    project
        .child(".claude/hooks/audit/user.sh")
        .write_str("user")
        .unwrap();
    project
        .child(".claude/settings.local.json")
        .write_str(r#"{"sentinel":true}"#)
        .unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("unmanaged"))
        .stderr(contains_path(".claude/hooks/audit"));
    project
        .child(".claude/settings.local.json")
        .assert(r#"{"sentinel":true}"#);
    project.child(".claude/hooks/audit/user.sh").assert("user");
    project
        .child(".mars/hooks/claude/audit")
        .assert(predicate::path::missing());
}

#[test]
fn blocking_hook_parent_fails_preflight_without_partial_state() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    project.child(".claude").create_dir_all().unwrap();
    project
        .child(".claude/hooks")
        .write_str("user file")
        .unwrap();

    sync(&project)
        .failure()
        .stderr(contains_path(".claude/hooks"))
        .stderr(predicate::str::contains("directory"));
    project.child(".claude/hooks").assert("user file");
    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::missing());
    project
        .child(".mars/hooks/claude/audit")
        .assert(predicate::path::missing());
}

#[test]
fn identical_unmanaged_hook_is_preserved_through_sync_and_removal() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    let installed = project.child(".claude/hooks/audit");
    installed.create_dir_all().unwrap();
    installed
        .child("hook.toml")
        .write_str("[targets.\".claude\"]\n")
        .unwrap();
    installed.child("run.sh").write_str("#!/bin/sh\n").unwrap();
    installed
        .child("claude.json")
        .write_str(r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#)
        .unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("unmanaged"))
        .stderr(contains_path(".claude/hooks/audit"));
    installed.child("run.sh").assert("#!/bin/sh\n");
    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::missing());

    fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
    sync(&project).success();
    installed.child("run.sh").assert("#!/bin/sh\n");
}

#[cfg(unix)]
#[test]
fn stale_managed_hook_recovers_after_transient_copy_failure() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"old"}]}]}"#,
    );
    project
        .child("hooks/audit/run.sh")
        .write_str("#!/bin/sh\nprintf old\n")
        .unwrap();
    sync(&project).success();

    project
        .child("hooks/audit/run.sh")
        .write_str("#!/bin/sh\nprintf new\n")
        .unwrap();
    let hooks_dir = project.child(".claude/hooks");
    let original_permissions = fs::metadata(hooks_dir.path()).unwrap().permissions();
    fs::set_permissions(hooks_dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let failed = sync(&project);
    fs::set_permissions(hooks_dir.path(), original_permissions).unwrap();
    failed
        .success()
        .stderr(predicate::str::contains("failed to copy"));
    project
        .child(".claude/hooks/audit/run.sh")
        .assert("#!/bin/sh\nprintf old\n");

    sync(&project).success();
    project
        .child(".claude/hooks/audit/run.sh")
        .assert("#!/bin/sh\nprintf new\n");
    project
        .child(".claude/settings.local.json")
        .assert(predicate::str::contains("SessionStart"));
}

#[test]
fn config_destination_directory_rejects_before_any_config_write() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    let mcp = project.child("mcp/audit");
    mcp.create_dir_all().unwrap();
    mcp.child("mcp.toml")
        .write_str("command = \"node\"\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    project
        .child(".claude/settings.local.json")
        .create_dir_all()
        .unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("settings.local.json"))
        .stderr(predicate::str::contains("file"));
    project
        .child(".claude/.mcp.json")
        .assert(predicate::path::missing());
    project
        .child(".mars/hooks/claude/audit")
        .assert(predicate::path::missing());
}

#[test]
fn skill_only_sync_ignores_unrelated_malformed_config() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "source",
        &[],
        &[("demo", "---\nname: demo\ndescription: demo\n---\n")],
    );
    let project = dir.child("project");
    let malformed_mcp = source.join("mcp").join("broken");
    fs::create_dir_all(&malformed_mcp).unwrap();
    fs::write(malformed_mcp.join("mcp.toml"), "this is not = valid = toml").unwrap();
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\"]\n[dependencies]\nsource = {{ path = \"{}\", only_skills = true }}\n",
            portable_path(&source)
        ))
        .unwrap();
    project.child(".claude").create_dir_all().unwrap();
    project
        .child(".claude/settings.local.json")
        .write_str("{broken,")
        .unwrap();

    sync(&project).success();
    project
        .child(".claude/settings.local.json")
        .assert("{broken,");
    project
        .child(".mars/skills/demo/SKILL.md")
        .assert(predicate::path::exists());
}

#[test]
fn exported_dependency_hook_uses_item_lifecycle_for_whole_directory_copy() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("base");
    source.create_dir_all().unwrap();
    write_hook(
        &source,
        "dep-hook",
        "visibility = \"exported\"\n[targets.\".claude\"]\n",
    );
    write_fragment(
        &source,
        "dep-hook",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf dep"}]}]}"#,
    );
    source
        .child("hooks/dep-hook/asset.txt")
        .write_str("copied verbatim")
        .unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\"]\n[dependencies]\nbase = {{ path = \"{}\" }}\n",
            portable_path(source.path())
        ))
        .unwrap();

    sync(&project).success();
    project
        .child(".claude/hooks/dep-hook/asset.txt")
        .assert("copied verbatim");
    let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
    assert!(lock.contains("hooks/claude/dep-hook"));
}

#[test]
fn unrelated_malformed_mcp_file_cannot_retain_removed_hook_ownership() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    sync(&project).success();

    fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
    project
        .child(".claude/.mcp.json")
        .write_str("{malformed")
        .unwrap();
    sync(&project).success();

    let lock: mars_agents::lock::LockFile =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    assert!(
        !lock
            .config_entries
            .get(".claude")
            .is_some_and(|records| records.contains_key("hook:SessionStart:audit")),
        "confirmed hook removal must not leave ghost ownership"
    );
    let settings: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(settings, serde_json::json!({}));
    project
        .child(".claude/hooks/audit")
        .assert(predicate::path::missing());

    // Repair and run again: the already-consistent state remains converged and
    // no ghost record is resurrected on a later sync.
    project.child(".claude/.mcp.json").write_str("{}").unwrap();
    sync(&project).success();
    assert_config_entry_consistency(&project);
    let repaired = fs::read_to_string(project.child("mars.lock").path()).unwrap();
    sync(&project).success();
    assert_eq!(
        fs::read_to_string(project.child("mars.lock").path()).unwrap(),
        repaired
    );
}
