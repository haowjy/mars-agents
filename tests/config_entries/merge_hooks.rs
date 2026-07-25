use super::support::*;

#[test]
fn nested_hook_container_is_ignored_consistently() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("source");
    let nested_hook = source.child("packages/tool/hooks/audit");
    nested_hook.create_dir_all().unwrap();
    nested_hook
        .child("hook.toml")
        .write_str("visibility = \"exported\"\n[targets.\".claude\"]\n")
        .unwrap();
    nested_hook
        .child("claude.json")
        .write_str(r#"{"SessionStart":[{"hooks":[{"type":"command","command":"echo nested"}]}]}"#)
        .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\"]\n[dependencies]\ntool = {{ path = \"{}\" }}\n",
            portable_path(source.path())
        ))
        .unwrap();

    sync(&project).success();
    project
        .child(".claude/hooks/audit")
        .assert(predicate::path::missing());
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
    let canonical_project = dunce::canonicalize(project.path()).unwrap();
    let expected = format!(
        "bash \"{}\"",
        portable_path(&canonical_project.join(".claude/hooks/audit/run.sh"))
    );
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
    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::missing());
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
#[cfg(unix)]
fn stale_hook_diagnostics_distinguish_planned_and_unconfirmed_removals() {
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
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf owned"}]}]}"#,
    );
    sync(&project).success();

    fs::rename(
        project.child("hooks/audit").path(),
        project.child("audit.removed").path(),
    )
    .unwrap();
    mars()
        .args(["sync", "--diff", "--root", project.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "target `.claude` has stale config entries: hook:SessionStart:audit",
        ));

    let target_dir = project.child(".claude");
    fs::set_permissions(target_dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let assertion = sync(&project);
    fs::set_permissions(target_dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assertion
        .success()
        .stderr(predicate::str::contains(
            "failed to remove prior hook entries",
        ))
        .stderr(predicate::str::contains("removed stale config entries").not());

    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::is_file());
    assert!(
        fs::read_to_string(project.child("mars.lock").path())
            .unwrap()
            .contains("hook:SessionStart:audit")
    );
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
