mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::fs;
use toml::Value;

use common::*;

fn write_hook(project: &assert_fs::fixture::ChildPath, name: &str, body: &str) {
    let hook = project.child("hooks").child(name);
    hook.create_dir_all().unwrap();
    hook.child("hook.toml").write_str(body).unwrap();
    hook.child("run.sh").write_str("#!/bin/sh\n").unwrap();
}

#[test]
fn native_hooks_emit_multiple_events_targets_and_optional_matchers() {
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
        r#"name = "audit"
[targets.".claude"]
events = ["PreToolUse", "PostToolUse"]
matcher = "Bash|Agent"
[targets.".codex"]
events = ["SessionStart"]
[action]
kind = "script"
path = "run.sh"
"#,
    );

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let claude: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(claude["hooks"]["PreToolUse"][0]["matcher"], "Bash|Agent");
    assert_eq!(claude["hooks"]["PostToolUse"][0]["matcher"], "Bash|Agent");

    let codex: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
    )
    .unwrap();
    assert!(codex["hooks"]["SessionStart"][0].get("matcher").is_none());
}

#[test]
fn old_hook_schema_is_a_hard_error_with_filename_and_hint() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project.child("mars.toml").write_str("").unwrap();
    write_hook(
        &project,
        "old",
        r#"name = "old"
event = "tool.pre"
targets = [".claude"]
[action]
kind = "script"
path = "run.sh"
"#,
    );

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hook.toml"))
        .stderr(predicate::str::contains("removed universal hook schema"))
        .stderr(predicate::str::contains("[targets.\".claude\"]"));
}

#[test]
fn unknown_event_lists_allowlist_and_escape_hatch_without_mutating_targets() {
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
    write_hook(
        &project,
        "future",
        r#"name = "future"
[targets.".claude"]
events = ["FutureEvent"]
[action]
kind = "script"
path = "run.sh"
"#,
    );

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("valid events: SessionStart"))
        .stderr(predicate::str::contains("unchecked = true"));
    assert_eq!(
        fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
        "{\"sentinel\":true}"
    );
}

#[test]
fn unchecked_event_warns_and_passes_through_verbatim() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();
    write_hook(
        &project,
        "future",
        r#"name = "future"
[targets.".claude"]
events = ["FutureEvent"]
unchecked = true
[action]
kind = "script"
path = "run.sh"
"#,
    );

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "passes unknown event `FutureEvent`",
        ));
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert!(json["hooks"]["FutureEvent"].is_array());
}

#[test]
fn one_sync_migrates_universal_hook_bindings_and_lock_to_native_events() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\", \".codex\"]\n")
        .unwrap();
    write_hook(
        &project,
        "context-autosync",
        r#"name = "context-autosync"
[targets.".claude"]
events = ["SessionEnd", "SubagentStop"]
[targets.".codex"]
events = ["Stop"]
[action]
kind = "script"
path = "run.sh"
"#,
    );

    // Bootstrap a structurally valid lock, then replace its native ownership
    // records and emitted configs with the shapes written by universal-era Mars.
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    for target in [".claude", ".codex"] {
        let records = lock["config_entries"][target].as_table_mut().unwrap();
        records.clear();
        records.insert(
            "hook:session.end:context-autosync".to_string(),
            toml::Table::from_iter([("source".to_string(), Value::String("_self".to_string()))])
                .into(),
        );
    }
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();

    let managed_command = format!(
        "bash '{}'",
        project
            .child("hooks/context-autosync/run.sh")
            .path()
            .display()
    );
    project
        .child(".claude/settings.local.json")
        .write_str(
            &serde_json::to_string_pretty(&serde_json::json!({
                "permissions": {"allow": ["Read(/tmp/user-owned/**)"]},
                "hooks": {
                    "SessionStop": [
                        {
                            "hooks": [{"type": "command", "command": managed_command}],
                        },
                        {
                            "matcher": "user-matcher",
                            "hooks": [{"type": "command", "command": "printf user-owned"}],
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
    project
        .child(".codex/hooks.json")
        .write_str(
            &serde_json::to_string_pretty(&serde_json::json!({
                "userSetting": true,
                "hooks": {
                    "SessionStop": [
                        {
                            "hooks": [{"type": "command", "command": managed_command}],
                        },
                        {
                            "hooks": [{"type": "command", "command": "printf user-owned"}],
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let claude: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        claude["permissions"]["allow"][0],
        "Read(/tmp/user-owned/**)"
    );
    assert_eq!(claude["hooks"]["SessionStop"].as_array().unwrap().len(), 1);
    assert_eq!(
        claude["hooks"]["SessionStop"][0]["hooks"][0]["command"],
        "printf user-owned"
    );
    for event in ["SessionEnd", "SubagentStop"] {
        let bindings = claude["hooks"][event].as_array().unwrap();
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("/hooks/context-autosync/")
        );
    }

    let codex: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(codex["userSetting"], true);
    assert_eq!(codex["hooks"]["SessionStop"].as_array().unwrap().len(), 1);
    let stop = codex["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1);
    assert!(
        stop[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("/hooks/context-autosync/")
    );

    let migrated_lock: Value =
        toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    let claude_records = migrated_lock["config_entries"][".claude"]
        .as_table()
        .unwrap();
    assert_eq!(claude_records.len(), 2);
    assert!(!claude_records.contains_key("hook:session.end:context-autosync"));
    assert!(claude_records.contains_key("hook:SessionEnd:context-autosync"));
    assert!(claude_records.contains_key("hook:SubagentStop:context-autosync"));
    let codex_records = migrated_lock["config_entries"][".codex"]
        .as_table()
        .unwrap();
    assert_eq!(codex_records.len(), 1);
    assert!(!codex_records.contains_key("hook:session.end:context-autosync"));
    assert!(codex_records.contains_key("hook:Stop:context-autosync"));
}

#[test]
fn plugin_only_hook_targets_are_hard_errors() {
    for (target, detail) in [
        (".opencode", "TypeScript plugins"),
        (".pi", "TypeScript extensions"),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project.child("mars.toml").write_str("").unwrap();
        write_hook(
            &project,
            "unsupported",
            &format!(
                r#"name = "unsupported"
[targets."{target}"]
events = ["anything"]
[action]
kind = "script"
path = "run.sh"
"#
            ),
        );
        mars()
            .args(["sync", "--root", project.path().to_str().unwrap()])
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "target `{target}` has no command-hook mechanism"
            )))
            .stderr(predicate::str::contains(detail));
    }
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
