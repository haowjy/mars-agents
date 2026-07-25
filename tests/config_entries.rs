mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::fs;
use toml::Value;

use common::*;

fn write_hook(project: &assert_fs::fixture::ChildPath, dir_name: &str, manifest: &str) {
    let hook = project.child("hooks").child(dir_name);
    hook.create_dir_all().unwrap();
    hook.child("hook.toml").write_str(manifest).unwrap();
    hook.child("run.sh").write_str("#!/bin/sh\n").unwrap();
}

fn write_fragment(project: &assert_fs::fixture::ChildPath, dir_name: &str, file: &str, json: &str) {
    project
        .child("hooks")
        .child(dir_name)
        .child(file)
        .write_str(json)
        .unwrap();
}

fn sync(project: &assert_fs::fixture::ChildPath) -> assert_cmd::assert::Assert {
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
}

fn write_dependency_hook(
    source: &assert_fs::fixture::ChildPath,
    name: &str,
    target: &str,
    event: &str,
    command: &str,
) {
    write_hook(
        source,
        name,
        &format!("visibility = \"exported\"\n[targets.\"{target}\"]\n"),
    );
    write_fragment(
        source,
        name,
        &format!("{}.json", target.trim_start_matches('.')),
        &format!(r#"{{"{event}":[{{"hooks":[{{"type":"command","command":"{command}"}}]}}]}}"#),
    );
}

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
            claude_source.path().display(),
            codex_source.path().display()
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
                claude_source.path().display()
            ));
        }
        if codex {
            raw.push_str(&format!(
                "codex-source = {{ path = \"{}\" }}\n",
                codex_source.path().display()
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

fn assert_hook_target_owner(
    project: &assert_fs::fixture::ChildPath,
    canonical_path: &str,
    target_root: &str,
) {
    let lock: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    let owner = lock["items"]
        .as_table()
        .unwrap()
        .values()
        .find(|item| {
            item["outputs"].as_array().is_some_and(|outputs| {
                outputs.iter().any(|output| {
                    output["target_root"].as_str() == Some(".mars")
                        && output["dest_path"].as_str() == Some(canonical_path)
                })
            })
        })
        .expect("canonical hook owner missing");
    assert!(
        owner["outputs"].as_array().unwrap().iter().any(|output| {
            output["target_root"].as_str() == Some(target_root)
                && output["dest_path"].as_str() == Some("hooks/audit")
        }),
        "{target_root} output was attached to the wrong scoped hook owner"
    );
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
        .stderr(predicate::str::contains(".claude/hooks/audit"));
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
        .stderr(predicate::str::contains(".claude/hooks"))
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
        .stderr(predicate::str::contains(".claude/hooks/audit"));
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
            source.display()
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
fn unmatched_legacy_claude_sweep_does_not_rewrite_settings() {
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
    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    lock["config_entries"][".claude"]["hook:SessionStart:audit"]
        .as_table_mut()
        .unwrap()
        .remove("emitted_json");
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    let original = r#"{"z":1,"a":2}"#;
    project
        .child(".claude/settings.json")
        .write_str(original)
        .unwrap();

    sync(&project).success();
    project.child(".claude/settings.json").assert(original);
}

#[test]
fn new_emission_never_name_matches_user_hooks_in_committed_claude_settings() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    project.child(".claude").create_dir_all().unwrap();
    let original = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"/user/hooks/audit/run.sh"}]}],"UserPromptSubmit":[]}}"#;
    project
        .child(".claude/settings.json")
        .write_str(original)
        .unwrap();
    write_hook(&project, "audit", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf managed"}]}]}"#,
    );

    sync(&project).success();
    project.child(".claude/settings.json").assert(original);
}

#[test]
fn malformed_merge_target_configs_fail_preflight_without_mutation() {
    for (target, config) in [
        (".claude", "settings.local.json"),
        (".codex", "hooks.json"),
        (".cursor", "hooks.json"),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
            .unwrap();
        project.child(target).create_dir_all().unwrap();
        project
            .child(target)
            .child(config)
            .write_str("{bad,")
            .unwrap();
        write_hook(&project, "audit", &format!("[targets.\"{target}\"]\n"));
        let (event, entry) = if target == ".cursor" {
            ("sessionStart", r#"{"command":"true"}"#)
        } else {
            (
                "SessionStart",
                r#"{"hooks":[{"type":"command","command":"true"}]}"#,
            )
        };
        write_fragment(
            &project,
            "audit",
            &format!("{}.json", target.trim_start_matches('.')),
            &format!(r#"{{"{event}":[{entry}]}}"#),
        );

        sync(&project)
            .failure()
            .stderr(predicate::str::contains(config))
            .stderr(predicate::str::contains("valid JSON"));
        project.child(target).child(config).assert("{bad,");
        project
            .child(format!(
                ".mars/hooks/{}/audit",
                target.trim_start_matches('.')
            ))
            .assert(predicate::path::missing());
    }
}

#[test]
fn malformed_legacy_claude_settings_fail_before_the_migration_sweep() {
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
    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    lock["config_entries"][".claude"]["hook:SessionStart:audit"]
        .as_table_mut()
        .unwrap()
        .remove("emitted_json");
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    project
        .child(".claude/settings.json")
        .write_str("{legacy-malformed,")
        .unwrap();
    let local_before =
        fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("settings.json"))
        .stderr(predicate::str::contains("valid JSON"));
    project
        .child(".claude/settings.json")
        .assert("{legacy-malformed,");
    assert_eq!(
        fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
        local_before
    );
}

#[test]
fn fragments_pass_through_native_entries_substitute_installed_paths_and_copy_hook_dirs() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\", \".codex\"]\n")
        .unwrap();
    write_hook(
        &project,
        "audit",
        r#"visibility = "exported"
order = 7
[targets.".claude"]
[targets.".codex"]
"#,
    );
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{
      "PreToolUse": [{"matcher":"Bash|Agent","hooks":[{"type":"command","command":"bash \"${MARS_HOOK_DIR}/run.sh\"","timeout":30}]}],
      "PostToolUse": [{"hooks":[{"type":"http","url":"https://example.test","timeout":9}]}]
    }"#,
    );
    write_fragment(
        &project,
        "audit",
        "codex.json",
        r#"{
      "SessionStart": [{"matcher":"Bash","hooks":[{"type":"command","command":"bash \"${MARS_HOOK_DIR}/run.sh\"","statusMessage":"audit"}]}]
    }"#,
    );

    sync(&project).success();
    let installed = project.child(".claude/hooks/audit");
    installed
        .child("hook.toml")
        .assert(predicate::path::exists());
    installed
        .child("claude.json")
        .assert(predicate::path::exists());
    installed.child("run.sh").assert(predicate::path::exists());

    let claude: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(claude["hooks"]["PreToolUse"][0]["matcher"], "Bash|Agent");
    assert_eq!(claude["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 30);
    let expected = format!("bash \"{}\"", installed.child("run.sh").path().display());
    assert_eq!(
        claude["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        expected
    );
    assert!(!expected.contains(".mars/staging"));
    assert_eq!(
        claude["hooks"]["PostToolUse"][0]["hooks"][0]["type"],
        "http"
    );

    let codex: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        codex["hooks"]["SessionStart"][0]["hooks"][0]["statusMessage"],
        "audit"
    );
    assert!(
        codex["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/.codex/hooks/audit/run.sh")
    );

    let lock: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    let emitted = lock["config_entries"][".claude"]["hook:PreToolUse:audit"]["emitted_json"]
        .as_str()
        .unwrap();
    assert!(emitted.contains("timeout"));
    assert!(emitted.contains("/.claude/hooks/audit/run.sh"));
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
            source.path().display()
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
fn wrapped_and_bare_fragments_emit_equal_entries() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(
        &project,
        "bare",
        "[targets.\".claude\"]\nfragment = \"bare.json\"\n",
    );
    write_hook(
        &project,
        "wrapped",
        "[targets.\".claude\"]\nfragment = \"wrapped.json\"\n",
    );
    let entry = r#"[{"matcher":"Bash","hooks":[{"type":"command","command":"printf ok"}]}]"#;
    write_fragment(
        &project,
        "bare",
        "bare.json",
        &format!(r#"{{"PreToolUse":{entry}}}"#),
    );
    write_fragment(
        &project,
        "wrapped",
        "wrapped.json",
        &format!(r#"{{"version":1,"description":"pasted","hooks":{{"PostToolUse":{entry}}}}}"#),
    );
    sync(&project).success();
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(json["hooks"]["PreToolUse"], json["hooks"]["PostToolUse"]);
}

#[test]
fn invalid_fragments_fail_preflight_without_mutation() {
    for (body, expected) in [
        ("not json", "not valid JSON"),
        (r#"{"PreToolUse": {"hooks":[]}}"#, "value must be an array"),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str("[settings]\ntargets = [\".claude\"]\n")
            .unwrap();
        project.child(".claude").create_dir_all().unwrap();
        project
            .child(".claude/settings.local.json")
            .write_str("{\"sentinel\":true}")
            .unwrap();
        write_hook(&project, "bad", "[targets.\".claude\"]\n");
        write_fragment(&project, "bad", "claude.json", body);
        sync(&project)
            .failure()
            .stderr(predicate::str::contains(expected));
        assert_eq!(
            fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
            "{\"sentinel\":true}"
        );
        assert!(!project.child(".mars/hooks/bad").exists());
    }
}

#[test]
fn unknown_event_is_strict_by_default_and_unchecked_warns_and_passes() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "future", "[targets.\".claude\"]\n");
    write_fragment(&project, "future", "claude.json", r#"{"FutureEvent":[]}"#);
    sync(&project)
        .failure()
        .stderr(predicate::str::contains("valid events: SessionStart"))
        .stderr(predicate::str::contains("unchecked = true"));
    project
        .child("hooks/future/hook.toml")
        .write_str("[targets.\".claude\"]\nunchecked = true\n")
        .unwrap();
    sync(&project).success().stderr(predicate::str::contains(
        "passes unknown event `FutureEvent`",
    ));
    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::missing());
}

#[test]
fn v011_schema_is_a_hard_error_with_filename_and_fragment_hint() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project.child("mars.toml").write_str("").unwrap();
    write_hook(
        &project,
        "old",
        r#"name = "old"
[targets.".claude"]
events = ["PreToolUse"]
matcher = "Bash"
[action]
kind = "script"
path = "run.sh"
"#,
    );
    sync(&project)
        .failure()
        .stderr(predicate::str::contains("hook.toml"))
        .stderr(predicate::str::contains("removed v0.11.0 hook schema"))
        .stderr(predicate::str::contains("native fragment"));
}

#[test]
fn name_defaults_to_directory_and_explicit_override_controls_install_and_key() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "defaulted", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "defaulted",
        "claude.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    write_hook(
        &project,
        "source-dir",
        "name = \"renamed\"\n[targets.\".claude\"]\n",
    );
    write_fragment(
        &project,
        "source-dir",
        "claude.json",
        r#"{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    sync(&project).success();
    project
        .child(".claude/hooks/defaulted")
        .assert(predicate::path::is_dir());
    project
        .child(".claude/hooks/renamed")
        .assert(predicate::path::is_dir());
    assert!(!project.child(".claude/hooks/source-dir").exists());
    let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
    assert!(lock.contains("hook:SessionStart:defaulted"));
    assert!(lock.contains("hook:Stop:renamed"));
}

#[test]
fn structural_ownership_preserves_edited_and_user_entries_and_prunes_emptied_events() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(&project, "owned", "[targets.\".claude\"]\n");
    write_fragment(
        &project,
        "owned",
        "claude.json",
        r#"{
      "PreToolUse":[{"hooks":[{"type":"command","command":"printf owned"}]}],
      "PostToolUse":[{"hooks":[{"type":"command","command":"printf prune"}]}]
    }"#,
    );
    sync(&project).success();
    let path = project.child(".claude/settings.local.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
    json["unrelated"] = serde_json::json!({"keep": true});
    json["hooks"]["UserPromptSubmit"] = serde_json::json!([]);
    json["hooks"]["PreToolUse"][0]["hooks"][0]["command"] = serde_json::json!("printf user-edited");
    json["hooks"]["PreToolUse"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"hooks":[{"type":"command","command":"printf user"}]}));
    path.write_str(&serde_json::to_string_pretty(&json).unwrap())
        .unwrap();
    fs::remove_dir_all(project.child("hooks/owned").path()).unwrap();
    sync(&project).success();
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
    assert_eq!(after["unrelated"]["keep"], true);
    assert_eq!(after["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    assert!(after["hooks"].get("PostToolUse").is_none());
    assert!(after["hooks"]["UserPromptSubmit"].is_array());
}

#[test]
fn edited_managed_entries_emit_divergence_warnings_on_every_merge_target() {
    for (target, event, original_entry, edited_command) in [
        (
            ".claude",
            "SessionStart",
            r#"{"hooks":[{"type":"command","command":"printf owned"}]}"#,
            "printf edited-claude",
        ),
        (
            ".codex",
            "SessionStart",
            r#"{"hooks":[{"type":"command","command":"printf owned"}]}"#,
            "printf edited-codex",
        ),
        (
            ".cursor",
            "sessionStart",
            r#"{"command":"printf owned"}"#,
            "printf edited-cursor",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
            .unwrap();
        write_hook(&project, "audit", &format!("[targets.\"{target}\"]\n"));
        write_fragment(
            &project,
            "audit",
            &format!("{}.json", target.trim_start_matches('.')),
            &format!(r#"{{"{event}":[{original_entry}]}}"#),
        );
        sync(&project).success();
        let config = if target == ".claude" {
            "settings.local.json"
        } else {
            "hooks.json"
        };
        let path = project.child(target).child(config);
        let mut json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
        if target == ".cursor" {
            json["hooks"][event][0]["command"] = edited_command.into();
        } else {
            json["hooks"][event][0]["hooks"][0]["command"] = edited_command.into();
        }
        path.write_str(&serde_json::to_string_pretty(&json).unwrap())
            .unwrap();

        sync(&project)
            .success()
            .stderr(predicate::str::contains("config-divergence"))
            .stderr(predicate::str::contains(target))
            .stderr(predicate::str::contains("audit"))
            .stderr(predicate::str::contains("exists locally").not());
        let after: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
        assert_eq!(after["hooks"][event].as_array().unwrap().len(), 2);
    }
}

#[test]
fn empty_fragment_events_never_create_merge_config_residue() {
    for (target, event, config) in [
        (".claude", "SessionStart", "settings.local.json"),
        (".codex", "SessionStart", "hooks.json"),
        (".cursor", "sessionStart", "hooks.json"),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
            .unwrap();
        project.child(target).create_dir_all().unwrap();
        project
            .child(target)
            .child(config)
            .write_str(r#"{"sentinel":true}"#)
            .unwrap();
        write_hook(&project, "empty", &format!("[targets.\"{target}\"]\n"));
        write_fragment(
            &project,
            "empty",
            &format!("{}.json", target.trim_start_matches('.')),
            &format!(r#"{{"{event}":[]}}"#),
        );

        sync(&project).success();
        let after: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(project.child(target).child(config).path()).unwrap(),
        )
        .unwrap();
        assert_eq!(after["sentinel"], true);
        assert!(after.get("hooks").is_none());
        fs::remove_dir_all(project.child("hooks/empty").path()).unwrap();
        sync(&project).success();
        let removed: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(project.child(target).child(config).path()).unwrap(),
        )
        .unwrap();
        assert!(removed.get("hooks").is_none());
    }
}

#[test]
fn one_sync_migrates_v011_command_path_residue_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\", \".codex\"]\n")
        .unwrap();
    write_hook(
        &project,
        "audit",
        "[targets.\".claude\"]\n[targets.\".codex\"]\n",
    );
    write_fragment(
        &project,
        "audit",
        "claude.json",
        r#"{"SessionEnd":[{"hooks":[{"type":"command","command":"bash \"${MARS_HOOK_DIR}/run.sh\""}]}]}"#,
    );
    write_fragment(
        &project,
        "audit",
        "codex.json",
        r#"{"Stop":[{"hooks":[{"type":"command","command":"bash \"${MARS_HOOK_DIR}/run.sh\""}]}]}"#,
    );
    sync(&project).success();
    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    for target in [".claude", ".codex"] {
        let records = lock["config_entries"][target].as_table_mut().unwrap();
        records.clear();
        records.insert(
            "hook:PreToolUse:audit".into(),
            toml::Table::from_iter([("source".into(), Value::String("_self".into()))]).into(),
        );
    }
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    let legacy = format!(
        "bash '{}'/hooks/audit/run.sh",
        project
            .child(".mars/staging/base/mars-native")
            .path()
            .display()
    );
    for path in [
        project.child(".claude/settings.local.json"),
        project.child(".codex/hooks.json"),
    ] {
        path.write_str(&serde_json::to_string_pretty(&serde_json::json!({"user":true,"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":legacy}]},{"hooks":[{"type":"command","command":"printf user"}]}]}})).unwrap()).unwrap();
    }
    sync(&project).success();
    for path in [
        project.child(".claude/settings.local.json"),
        project.child(".codex/hooks.json"),
    ] {
        let raw = fs::read_to_string(path.path()).unwrap();
        assert!(!raw.contains(".mars/staging"));
        assert!(raw.contains("printf user"));
        assert!(raw.contains("\"user\": true"));
    }
    let first_claude =
        fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap();
    let first_codex = fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap();
    let first_lock = fs::read_to_string(lock_path.path()).unwrap();
    sync(&project).success();
    assert_eq!(
        fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
        first_claude
    );
    assert_eq!(
        fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
        first_codex
    );
    assert_eq!(fs::read_to_string(lock_path.path()).unwrap(), first_lock);
}

#[test]
fn legacy_opencode_hook_records_reach_the_removal_only_sweep() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".opencode\"]\n")
        .unwrap();
    write_hook(
        &project,
        "audit",
        "[targets.\".opencode\"]\nfragment = \"plugin.ts\"\n",
    );
    write_fragment(&project, "audit", "plugin.ts", "export default {}");
    sync(&project).success();

    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    let records = lock["config_entries"][".opencode"].as_table_mut().unwrap();
    records.clear();
    records.insert(
        "hook:tool.pre:audit".into(),
        toml::Table::from_iter([("source".into(), Value::String("_self".into()))]).into(),
    );
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    project
        .child(".opencode/opencode.json")
        .write_str(
            r#"{"user":true,"hooks":{"tool.pre":["/cache/hooks/audit/run.sh","printf user"]}}"#,
        )
        .unwrap();
    fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();

    sync(&project).success();
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".opencode/opencode.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(json["user"], true);
    assert_eq!(
        json["hooks"]["tool.pre"],
        serde_json::json!(["printf user"])
    );
}

#[test]
fn cursor_fragments_emit_flat_versioned_entries_and_preserve_user_entries() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".cursor\"]\n")
        .unwrap();
    project.child(".cursor").create_dir_all().unwrap();
    project
        .child(".cursor/hooks.json")
        .write_str(r#"{"version":99,"hooks":{"sessionStart":[{"command":"printf user"}]}}"#)
        .unwrap();
    write_hook(&project, "audit", "[targets.\".cursor\"]\n");
    write_fragment(
        &project,
        "audit",
        "cursor.json",
        r#"{"version":1,"hooks":{
          "beforeShellExecution":[{"command":"bash \"${MARS_HOOK_DIR}/run.sh\"","matcher":"^git ","timeout":5,"futureField":true}],
          "postToolUse":[{"command":"printf managed"}]
        }}"#,
    );

    sync(&project).success();
    let path = project.child(".cursor/hooks.json");
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["hooks"]["sessionStart"][0]["command"], "printf user");
    let emitted = &json["hooks"]["beforeShellExecution"][0];
    assert_eq!(emitted["matcher"], "^git ");
    assert_eq!(emitted["futureField"], true);
    assert!(emitted.get("hooks").is_none());
    assert!(
        emitted["command"]
            .as_str()
            .unwrap()
            .contains("/.cursor/hooks/audit/run.sh")
    );

    let mut edited = json;
    edited["hooks"]["postToolUse"][0]["command"] = serde_json::json!("printf user-edited");
    path.write_str(&serde_json::to_string_pretty(&edited).unwrap())
        .unwrap();
    fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
    sync(&project).success();
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path.path()).unwrap()).unwrap();
    assert!(after["hooks"].get("beforeShellExecution").is_none());
    assert_eq!(after["hooks"]["sessionStart"][0]["command"], "printf user");
    assert_eq!(
        after["hooks"]["postToolUse"][0]["command"],
        "printf user-edited"
    );
}

#[test]
fn cursor_allowlist_is_camel_case_and_unchecked_passes_unknown_events() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".cursor\"]\n")
        .unwrap();
    write_hook(&project, "future", "[targets.\".cursor\"]\n");
    write_fragment(
        &project,
        "future",
        "cursor.json",
        r#"{"FutureEvent":[{"command":"true"}]}"#,
    );
    sync(&project)
        .failure()
        .stderr(predicate::str::contains(
            "valid events: beforeShellExecution, beforeMCPExecution",
        ))
        .stderr(predicate::str::contains("sessionStart"))
        .stderr(predicate::str::contains("preToolUse"));
    project
        .child("hooks/future/hook.toml")
        .write_str("[targets.\".cursor\"]\nunchecked = true\n")
        .unwrap();
    sync(&project).success();
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".cursor/hooks.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(json["hooks"]["FutureEvent"][0]["command"], "true");
}

#[test]
fn file_fragments_place_substitute_remove_and_resync_idempotently() {
    for (target, destination, export) in [
        (
            ".opencode",
            "plugins/mars-audit.ts",
            r#"import type { Plugin } from "@opencode-ai/plugin"
const SCRIPT = "${MARS_HOOK_DIR}/run.sh"
export const Audit: Plugin = async ({ $ }) => ({
  "tool.execute.before": async () => { await $`bash ${SCRIPT}` }
})"#,
        ),
        (
            ".pi",
            "extensions/mars-audit.ts",
            r#"import type { ExtensionAPI } from "@mariozechner/pi-coding-agent"
const SCRIPT = "${MARS_HOOK_DIR}/run.sh"
export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async () => { await pi.exec("bash", [SCRIPT]) })
}"#,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
            .unwrap();
        write_hook(
            &project,
            "audit",
            &format!("[targets.\"{target}\"]\nfragment = \"plugin.ts\"\n"),
        );
        write_fragment(&project, "audit", "plugin.ts", export);
        sync(&project).success();
        let placed = project.child(target).child(destination);
        let first = fs::read_to_string(placed.path()).unwrap();
        assert!(!first.contains("${MARS_HOOK_DIR}"));
        assert!(first.contains(&format!("{target}/hooks/audit/run.sh")));
        let first_lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
        sync(&project).success();
        assert_eq!(fs::read_to_string(placed.path()).unwrap(), first);
        assert_eq!(
            fs::read_to_string(project.child("mars.lock").path()).unwrap(),
            first_lock
        );
        project
            .child(target)
            .child(if target == ".opencode" {
                "plugins/user.ts"
            } else {
                "extensions/user.ts"
            })
            .write_str("user")
            .unwrap();
        fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
        sync(&project).success();
        placed.assert(predicate::path::missing());
        project
            .child(target)
            .child(if target == ".opencode" {
                "plugins/user.ts"
            } else {
                "extensions/user.ts"
            })
            .assert("user");
    }
}

#[test]
fn file_fragment_preflight_errors_do_not_mutate_and_unchecked_is_rejected() {
    for manifest in [
        "[targets.\".opencode\"]\nfragment = \"missing.ts\"\n",
        "[targets.\".opencode\"]\nfragment = \"plugin.ts\"\nunchecked = false\n",
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str("[settings]\ntargets = [\".opencode\"]\n")
            .unwrap();
        project.child(".opencode/plugins").create_dir_all().unwrap();
        project
            .child(".opencode/plugins/sentinel.ts")
            .write_str("sentinel")
            .unwrap();
        write_hook(&project, "bad", manifest);
        if manifest.contains("plugin.ts") {
            write_fragment(&project, "bad", "plugin.ts", "export default {}");
        }
        let assertion = sync(&project).failure();
        if manifest.contains("missing.ts") {
            assertion.stderr(predicate::str::contains("failed to read fragment"));
        } else {
            assertion.stderr(predicate::str::contains("`unchecked` is not supported"));
        }
        project
            .child(".opencode/plugins/sentinel.ts")
            .assert("sentinel");
        project
            .child(".opencode/plugins/mars-bad.ts")
            .assert(predicate::path::missing());
        assert!(!project.child(".mars/hooks/bad").exists());
    }
}

#[test]
fn all_merge_destinations_are_validated_before_the_first_write() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    project.child(".claude").create_dir_all().unwrap();
    project
        .child(".claude/.mcp.json")
        .write_str(r#"{"sentinel":"mcp"}"#)
        .unwrap();
    project
        .child(".claude/settings.local.json")
        .write_str(r#"{"hooks":[]}"#)
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

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("settings.local.json"))
        .stderr(predicate::str::contains("hooks is not an object"));
    project
        .child(".claude/.mcp.json")
        .assert(r#"{"sentinel":"mcp"}"#);
    project
        .child(".claude/settings.local.json")
        .assert(r#"{"hooks":[]}"#);
    project
        .child(".mars/hooks/claude/audit")
        .assert(predicate::path::missing());
}

#[test]
fn codex_trust_docs_warn_that_script_edits_do_not_reprompt() {
    let docs = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/config/mcp-and-hooks.md"
    ))
    .unwrap();
    assert!(docs.contains("script contents"));
    assert!(docs.contains("without another trust prompt"));
}

#[test]
fn remove_prunes_stale_config_entries() {
    let dir = TempDir::new().unwrap();
    let source = create_mcp_source(&dir, "base", "context7");
    let project = dir.child("project");

    mars()
        .args(["init", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    mars()
        .args([
            "link",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();
    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let mcp_path = project.child(".claude").child(".mcp.json");
    let installed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path.path()).unwrap()).unwrap();
    assert!(installed["mcpServers"]["context7"].is_object());

    mars()
        .args(["remove", "base", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let removed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path.path()).unwrap()).unwrap();
    assert!(removed["mcpServers"]["context7"].is_null());
}

#[test]
fn override_writes_local_config() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Coder")], &[]);
    let override_path = create_source(
        &dir,
        "local-override",
        &[("coder", "# Local coder override")],
        &[],
    );

    let _agents_dir = dir.child("project").child(".agents");
    mars()
        .args([
            "init",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "override",
            "base",
            "--path",
            override_path.to_str().unwrap(),
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("override"));

    // mars.local.toml should exist
    assert!(dir.child("project").child("mars.local.toml").exists());

    let content = fs::read_to_string(dir.child("project").child("mars.local.toml").path()).unwrap();
    assert!(content.contains("base"));
    assert!(content.contains("local-override"));
}

#[test]
fn unlink_preserves_unrelated_config_sections() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(
            r#"
[package]
name = "sample"
version = "0.1.0"

[dependencies.base]
url = "https://github.com/org/base.git"
version = "v1.0"
agents = ["coder"]

[settings]
targets = [".claude"]
"#,
        )
        .unwrap();

    mars()
        .args([
            "unlink",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("from settings"));

    let config: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.toml").path()).unwrap()).unwrap();
    assert_eq!(config["package"]["name"].as_str(), Some("sample"));
    assert_eq!(
        config["dependencies"]["base"]["url"].as_str(),
        Some("https://github.com/org/base.git")
    );
    assert_eq!(
        config["dependencies"]["base"]["version"].as_str(),
        Some("v1.0")
    );
    assert_eq!(
        config["dependencies"]["base"]["agents"][0].as_str(),
        Some("coder")
    );
    assert!(
        config["settings"]
            .as_table()
            .is_some_and(|settings| !settings.contains_key("targets"))
    );
}

#[test]
fn unlink_clears_matching_managed_root() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project.child(".agents").create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(
            r#"
[settings]
managed_root = ".agents"
"#,
        )
        .unwrap();

    mars()
        .args([
            "unlink",
            ".agents",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed managed target `.agents`"));

    let config: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.toml").path()).unwrap()).unwrap();
    assert!(
        config["settings"]
            .as_table()
            .is_some_and(|settings| !settings.contains_key("managed_root"))
    );
    assert!(!project.child(".agents").exists());
}

#[test]
fn link_agents_prints_single_deprecation_warning() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\n")
        .unwrap();

    mars()
        .args([
            "link",
            ".agents",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("deprecated link target").count(1));
}
