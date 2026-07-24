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
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap(),
    )
    .unwrap();
    assert!(json["hooks"]["FutureEvent"].is_array());
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
        r#"{"SessionStart":[]}"#,
    );
    write_hook(
        &project,
        "source-dir",
        "name = \"renamed\"\n[targets.\".claude\"]\n",
    );
    write_fragment(&project, "source-dir", "claude.json", r#"{"Stop":[]}"#);
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
fn later_phase_hook_targets_keep_existing_no_mechanism_errors() {
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
            &format!("[targets.\"{target}\"]\nfragment = \"plugin.ts\"\n"),
        );
        write_fragment(&project, "unsupported", "plugin.ts", "export default {};");
        sync(&project)
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
