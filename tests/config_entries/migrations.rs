use super::support::*;

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
fn malformed_opencode_legacy_sweep_cannot_replace_file_hook() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    configure_file_fragment(&project, ".opencode");
    sync(&project).success();
    let plugin = project.child(".opencode/plugins/mars-audit.ts");
    let original = fs::read_to_string(plugin.path()).unwrap();

    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    lock.as_table_mut()
        .unwrap()
        .entry("config_entries")
        .or_insert_with(|| Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap()
        .entry(".opencode")
        .or_insert_with(|| Value::Table(toml::Table::new()))
        .as_table_mut()
        .unwrap()
        .insert(
            "hook:PreToolUse:audit".into(),
            toml::Table::from_iter([("source".into(), Value::String("_self".into()))]).into(),
        );
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    project
        .child("hooks/audit/plugin.ts")
        .write_str("export default 'replacement'\n")
        .unwrap();
    project
        .child(".opencode/opencode.json")
        .write_str("{malformed")
        .unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains("opencode.json"))
        .stderr(predicate::str::contains("valid JSON"));
    plugin.assert(original.as_str());
    let after: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    assert!(after["config_entries"][".opencode"]["hook:PreToolUse:audit"].is_table());
}
