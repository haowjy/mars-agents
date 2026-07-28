use super::support::*;

fn write_legacy_hook_source(dir: &TempDir, source_name: &str) -> std::path::PathBuf {
    let source = dir.child(source_name);
    source.create_dir_all().unwrap();
    source
        .child("mars.toml")
        .write_str(&format!(
            "[package]\nname = \"{source_name}\"\nversion = \"0.8.9\"\n"
        ))
        .unwrap();
    write_hook(
        &source,
        "context-autosync",
        "name = \"context-autosync\"\nevent = \"session.end\"\nvisibility = \"exported\"\ntargets = [\".claude\"]\n\n[action]\nkind = \"script\"\npath = \"run.sh\"\n",
    );
    source.to_path_buf()
}

fn write_migrated_hook_source(dir: &TempDir, source_name: &str) -> std::path::PathBuf {
    let source = dir.child(source_name);
    source.create_dir_all().unwrap();
    source
        .child("mars.toml")
        .write_str(&format!(
            "[package]\nname = \"{source_name}\"\nversion = \"0.9.0\"\n"
        ))
        .unwrap();
    write_hook(
        &source,
        "context-autosync",
        "name = \"context-autosync\"\nvisibility = \"exported\"\n\n[targets.\".claude\"]\n",
    );
    write_fragment(
        &source,
        "context-autosync",
        "claude.json",
        r#"{"SessionEnd":[{"hooks":[{"type":"command","command":"true"}]}]}"#,
    );
    source.to_path_buf()
}

fn write_named_hook_source(
    dir: &TempDir,
    source_name: &str,
    hook_name: &str,
    command: &str,
) -> std::path::PathBuf {
    let source = dir.child(source_name);
    source.create_dir_all().unwrap();
    source
        .child("mars.toml")
        .write_str(&format!(
            "[package]\nname = \"{source_name}\"\nversion = \"1.0.0\"\n"
        ))
        .unwrap();
    write_dependency_hook(&source, hook_name, ".claude", "SessionEnd", command);
    source.to_path_buf()
}

fn replace_hook_with_removed_schema(source: &std::path::Path, hook_name: &str) {
    let hook = source.join("hooks").join(hook_name);
    fs::remove_dir_all(&hook).unwrap();
    fs::create_dir_all(&hook).unwrap();
    fs::write(
        hook.join("hook.toml"),
        format!(
            "name = \"{hook_name}\"\nevent = \"session.end\"\nvisibility = \"exported\"\n\
             targets = [\".claude\"]\n\n[action]\nkind = \"script\"\npath = \"run.sh\"\n"
        ),
    )
    .unwrap();
    fs::write(hook.join("run.sh"), "#!/bin/sh\n").unwrap();
}

fn write_old_staging_fixture(project: &assert_fs::fixture::ChildPath) {
    let staged = project.child(".mars/staging/base/claude/hooks/context-autosync");
    staged.create_dir_all().unwrap();
    staged
        .child("hook.toml")
        .write_str(
            "name = \"context-autosync\"\nevent = \"session.end\"\n\
             visibility = \"exported\"\ntargets = [\".claude\"]\n\n\
             [action]\nkind = \"script\"\npath = \"run.sh\"\n",
        )
        .unwrap();
}

#[test]
fn upgrade_converges_in_one_shot_when_newest_source_is_readable() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("base-git");
    source.create_dir_all().unwrap();
    source
        .child("mars.toml")
        .write_str("[package]\nname = \"base\"\nversion = \"0.8.9\"\n")
        .unwrap();
    write_hook(
        &source,
        "context-autosync",
        "name = \"context-autosync\"\nevent = \"session.end\"\nvisibility = \"exported\"\n\
         targets = [\".claude\"]\n\n[action]\nkind = \"script\"\npath = \"run.sh\"\n",
    );
    for args in [
        vec!["init"],
        vec!["config", "user.email", "mars@example.com"],
        vec!["config", "user.name", "Mars Tests"],
        vec!["add", "."],
        vec!["commit", "-m", "legacy"],
        vec!["tag", "v0.8.9"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(source.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    fs::remove_dir_all(source.child("hooks/context-autosync").path()).unwrap();
    source
        .child("mars.toml")
        .write_str("[package]\nname = \"base\"\nversion = \"0.9.0\"\n")
        .unwrap();
    write_dependency_hook(&source, "context-autosync", ".claude", "SessionEnd", "true");
    for args in [
        vec!["add", "."],
        vec!["commit", "-m", "native hooks"],
        vec!["tag", "v0.9.0"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(source.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    let project = dir.child("project-upgrade-converges");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\nurl = \"file://{}\"\nversion = \">=0.8.9\"\n\n\
             [settings]\ntargets = [\".claude\"]\n",
            portable_path(source.path())
        ))
        .unwrap();
    project
        .child("mars.lock")
        .write_str("version = 2\n")
        .unwrap();

    mars()
        .args(["upgrade", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
    assert!(lock.contains("version = 3"));
    assert!(lock.contains("0.9.0"));
    project
        .child(".claude/hooks/context-autosync")
        .assert(predicate::path::is_dir());
}

#[test]
fn recovery_commands_converge_or_halt_at_the_reader_compiler_boundary() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let migrated = write_migrated_hook_source(&dir, "base-migrated");

    for (name, args, converges) in [
        ("upgrade", vec!["upgrade"], false),
        ("upgrade-bump", vec!["upgrade", "--bump"], false),
        ("repair", vec!["repair"], false),
        (
            "override",
            vec!["override", "base", "--path", migrated.to_str().unwrap()],
            true,
        ),
        ("remove", vec!["remove", "base"], true),
    ] {
        let project = dir.child(format!("project-{name}"));
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!(
                "[dependencies.base]\npath = \"{}\"\n\n[settings]\ntargets = [\".claude\"]\n",
                portable_path(&legacy)
            ))
            .unwrap();
        project
            .child("mars.lock")
            .write_str("version = 2\n")
            .unwrap();
        write_old_staging_fixture(&project);

        let assertion = mars()
            .args(args)
            .args(["--root", project.path().to_str().unwrap()])
            .assert();
        if converges {
            assertion.success();
            let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
            assert!(lock.contains("version = 3"));
        } else {
            let assertion = assertion
                .code(2)
                .stderr(predicate::str::contains(
                    "recovery halted before materialization",
                ))
                .stderr(predicate::str::contains("base@0.8.9"))
                .stderr(predicate::str::contains("then run"));
            if name == "upgrade" {
                assertion
                    .stderr(predicate::str::contains(
                        "newest compatible `base@0.8.9` still uses the removed hook schema; \
                         try --bump to escape the version constraint, or override/remove",
                    ))
                    .stderr(predicate::str::contains("mars upgrade base --bump"));
            } else if name == "upgrade-bump" {
                assertion
                    .stderr(predicate::str::contains("newest available"))
                    .stderr(predicate::str::contains(
                        "mars override base --path <path> or mars remove base",
                    ));
            }
            assert_eq!(
                fs::read_to_string(project.child("mars.lock").path()).unwrap(),
                "version = 2\n"
            );
        }
    }
}

#[test]
fn recovery_halt_json_lists_persisted_intent_and_every_blocker() {
    let dir = TempDir::new().unwrap();
    let source_a = write_legacy_hook_source(&dir, "a");
    let source_b = write_legacy_hook_source(&dir, "b");
    write_dependency_hook(
        &dir.child("b"),
        "readable-sibling",
        ".claude",
        "SessionEnd",
        "true",
    );
    let project = dir.child("project-json");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.a]\npath = \"{}\"\n\n[dependencies.b]\npath = \"{}\"\n",
            portable_path(&source_a),
            portable_path(&source_b),
        ))
        .unwrap();

    let output = mars()
        .args([
            "remove",
            "a",
            "--json",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["ok"], false);
    assert!(
        report["recovery_halt"]["persisted"][0]
            .as_str()
            .unwrap()
            .contains("removal")
    );
    let blockers = report["recovery_halt"]["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0]["package"], "b");
    assert_eq!(blockers[0]["version"], "0.8.9");
    assert_eq!(blockers[0]["hook_names"][0], "context-autosync");
    assert_eq!(blockers[0]["hook_names"].as_array().unwrap().len(), 1);
}

#[test]
fn normal_sync_reports_removed_hook_schema_by_source_package_and_version() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n\n[settings]\ntargets = [\".claude\"]\n",
            portable_path(&legacy)
        ))
        .unwrap();
    write_old_staging_fixture(&project);

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "source package `base` version `0.8.9`",
        ))
        .stderr(predicate::str::contains("removed v0.11.0 hook schema"))
        .stderr(predicate::str::contains(
            "suggested: `mars upgrade base --bump` or `mars remove base`",
        ))
        .stderr(predicate::str::contains(".mars/staging").not());
}

#[test]
fn filtered_transitive_removed_hook_error_hides_staging_path() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let workflow = dir.child("workflow");
    workflow.create_dir_all().unwrap();
    workflow
        .child("mars.toml")
        .write_str(&format!(
            "[package]\nname = \"workflow\"\nversion = \"1.0.0\"\n\n\
             [dependencies.base]\npath = \"{}\"\nagents = [\"missing\"]\n",
            portable_path(&legacy)
        ))
        .unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.workflow]\npath = \"{}\"\n\n\
             [settings]\ntargets = [\".claude\"]\n",
            portable_path(workflow.path())
        ))
        .unwrap();

    sync(&project)
        .failure()
        .stderr(predicate::str::contains(
            "source package `base` version `0.8.9`",
        ))
        .stderr(predicate::str::contains("removed v0.11.0 hook schema"))
        .stderr(predicate::str::contains(".mars/staging").not());
}

#[test]
fn sync_force_still_rejects_removed_hook_schema() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n\n[settings]\ntargets = [\".claude\"]\n",
            portable_path(&legacy)
        ))
        .unwrap();

    sync_force(&project)
        .failure()
        .stderr(predicate::str::contains("removed v0.11.0 hook schema"));
}

#[test]
fn remove_halt_persists_intent_and_leaves_installed_state_byte_identical() {
    let dir = TempDir::new().unwrap();
    let source_a = write_named_hook_source(&dir, "a", "audit-a", "printf a");
    let source_b = write_named_hook_source(&dir, "b", "audit-b", "printf b");
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.a]\npath = \"{}\"\n\n[dependencies.b]\npath = \"{}\"\n\n\
             [settings]\ntargets = [\".claude\"]\n",
            portable_path(&source_a),
            portable_path(&source_b),
        ))
        .unwrap();
    sync(&project).success();

    let settings_before = fs::read(project.child(".claude/settings.local.json").path()).unwrap();
    let lock_before = fs::read(project.child("mars.lock").path()).unwrap();
    let canonical_a_before =
        fs::read(project.child(".mars/hooks/claude/audit-a/run.sh").path()).unwrap();
    let canonical_b_before =
        fs::read(project.child(".mars/hooks/claude/audit-b/run.sh").path()).unwrap();
    let target_a_before = fs::read(project.child(".claude/hooks/audit-a/run.sh").path()).unwrap();
    let target_b_before = fs::read(project.child(".claude/hooks/audit-b/run.sh").path()).unwrap();
    replace_hook_with_removed_schema(&source_a, "audit-a");
    replace_hook_with_removed_schema(&source_b, "audit-b");

    mars()
        .args(["remove", "a", "--root", project.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("blocked by b@1.0.0"))
        .stderr(predicate::str::contains("hooks: audit-b"));

    let config_after = fs::read_to_string(project.child("mars.toml").path()).unwrap();
    assert!(!config_after.contains("[dependencies.a]"));
    assert!(config_after.contains("[dependencies.b]"));
    assert_eq!(
        fs::read(project.child(".claude/settings.local.json").path()).unwrap(),
        settings_before
    );
    assert_eq!(
        fs::read(project.child("mars.lock").path()).unwrap(),
        lock_before
    );
    assert_eq!(
        fs::read(project.child(".mars/hooks/claude/audit-a/run.sh").path()).unwrap(),
        canonical_a_before
    );
    assert_eq!(
        fs::read(project.child(".mars/hooks/claude/audit-b/run.sh").path()).unwrap(),
        canonical_b_before
    );
    assert_eq!(
        fs::read(project.child(".claude/hooks/audit-a/run.sh").path()).unwrap(),
        target_a_before
    );
    assert_eq!(
        fs::read(project.child(".claude/hooks/audit-b/run.sh").path()).unwrap(),
        target_b_before
    );
}

#[test]
fn dry_run_recovery_halt_persists_nothing() {
    let dir = TempDir::new().unwrap();
    let source_a = write_named_hook_source(&dir, "a", "audit-a", "printf a");
    let source_b = write_named_hook_source(&dir, "b", "audit-b", "printf b");
    let project = dir.child("project-dry-run");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.a]\npath = \"{}\"\n\n[dependencies.b]\npath = \"{}\"\n\n\
             [settings]\ntargets = [\".claude\"]\n",
            portable_path(&source_a),
            portable_path(&source_b),
        ))
        .unwrap();
    sync(&project).success();
    replace_hook_with_removed_schema(&source_b, "audit-b");

    let config_before = fs::read(project.child("mars.toml").path()).unwrap();
    let lock_before = fs::read(project.child("mars.lock").path()).unwrap();
    let target_before = fs::read(project.child(".claude/hooks/audit-b/run.sh").path()).unwrap();
    let context = mars_agents::types::MarsContext {
        project_root: project.to_path_buf(),
        meridian_managed: false,
    };
    let request = mars_agents::sync::SyncRequest {
        resolution: mars_agents::sync::ResolutionMode::Normal,
        mutation: Some(mars_agents::sync::ConfigMutation::RemoveDependency {
            name: mars_agents::types::SourceName::from("a"),
        }),
        options: mars_agents::sync::SyncOptions {
            dry_run: true,
            ..Default::default()
        },
        recovery: mars_agents::sync::RecoveryPolicy::DeferOnUnreadable,
        lossiness_mode: mars_agents::diagnostic::LossinessMode::Hidden,
    };

    let report = mars_agents::sync::execute(&context, &request).unwrap();
    assert!(report.recovery_halt.is_some());
    assert!(report.recovery_halt.unwrap().persisted[0].contains("would persist"));
    assert_eq!(
        fs::read(project.child("mars.toml").path()).unwrap(),
        config_before
    );
    assert_eq!(
        fs::read(project.child("mars.lock").path()).unwrap(),
        lock_before
    );
    assert_eq!(
        fs::read(project.child(".claude/hooks/audit-b/run.sh").path()).unwrap(),
        target_before
    );
}

#[test]
fn repair_halt_preserves_user_edited_target_and_all_persisted_state() {
    let dir = TempDir::new().unwrap();
    let source = write_named_hook_source(&dir, "base", "audit", "printf frozen");
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n\n[settings]\ntargets = [\".claude\"]\n",
            portable_path(&source)
        ))
        .unwrap();
    sync(&project).success();

    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    lock["version"] = Value::Integer(2);
    lock["items"]
        .as_table_mut()
        .unwrap()
        .retain(|_, item| item["kind"].as_str() != Some("hook"));
    lock_path
        .write_str(&toml::to_string_pretty(&lock).unwrap())
        .unwrap();
    replace_hook_with_removed_schema(&source, "audit");
    project
        .child(".claude/hooks/audit/run.sh")
        .write_str("#!/bin/sh\nprintf user-edit\n")
        .unwrap();
    let edited_target = fs::read(project.child(".claude/hooks/audit/run.sh").path()).unwrap();
    let settings_before = fs::read(project.child(".claude/settings.local.json").path()).unwrap();
    let lock_before = fs::read(lock_path.path()).unwrap();
    let canonical_before =
        fs::read(project.child(".mars/hooks/claude/audit/run.sh").path()).unwrap();

    mars()
        .args(["repair", "--root", project.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nothing persisted"))
        .stderr(predicate::str::contains("cannot repair"))
        .stderr(predicate::str::contains("blocked by base@1.0.0"));

    assert_eq!(
        fs::read(project.child(".claude/hooks/audit/run.sh").path()).unwrap(),
        edited_target
    );
    assert_eq!(
        fs::read(project.child(".claude/settings.local.json").path()).unwrap(),
        settings_before
    );
    assert_eq!(fs::read(lock_path.path()).unwrap(), lock_before);
    assert_eq!(
        fs::read(project.child(".mars/hooks/claude/audit/run.sh").path()).unwrap(),
        canonical_before
    );
}

#[test]
fn repair_halt_preserves_corrupt_lock_bytes() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n\n[settings]\ntargets = [\".claude\"]\n",
            portable_path(&legacy)
        ))
        .unwrap();
    let corrupt_lock = b"corrupt lock evidence\nnot = [valid";
    project
        .child("mars.lock")
        .write_binary(corrupt_lock)
        .unwrap();

    mars()
        .args([
            "repair",
            "--json",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "\"persisted\":[\"nothing persisted\"]",
        ));

    assert_eq!(
        fs::read(project.child("mars.lock").path()).unwrap(),
        corrupt_lock
    );
}

#[test]
fn override_can_replace_a_transitive_source_during_recovery() {
    let dir = TempDir::new().unwrap();
    let legacy = write_legacy_hook_source(&dir, "base");
    let migrated = write_migrated_hook_source(&dir, "base-migrated");
    let workflow = dir.child("workflow");
    workflow.create_dir_all().unwrap();
    workflow
        .child("mars.toml")
        .write_str(&format!(
            "[package]\nname = \"workflow\"\nversion = \"1.0.0\"\n\n\
             [dependencies.base]\npath = \"{}\"\n",
            portable_path(&legacy)
        ))
        .unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.workflow]\npath = \"{}\"\n\n\
             [settings]\ntargets = [\".claude\"]\n",
            portable_path(&workflow)
        ))
        .unwrap();
    project
        .child("mars.lock")
        .write_str("version = 2\n")
        .unwrap();
    write_old_staging_fixture(&project);

    mars()
        .args([
            "override",
            "base",
            "--path",
            migrated.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let local = fs::read_to_string(project.child("mars.local.toml").path()).unwrap();
    assert!(local.contains("[overrides.base]"));
    assert!(local.contains("base-migrated"));
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    let settings = fs::read_to_string(project.child(".claude/settings.local.json").path()).unwrap();
    assert!(settings.contains("SessionEnd"));
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
fn promoted_v2_lock_sweeps_v011_command_path_residue_and_is_idempotent() {
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
    downgrade_lock_to_v2(&project);
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

#[cfg(unix)]
#[test]
fn unconfirmed_opencode_legacy_sweep_suppresses_replacement_file_hook() {
    use std::os::unix::fs::PermissionsExt;

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
    let prior_lock: mars_agents::lock::LockFile =
        toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    let prior_record = prior_lock.config_entries[".opencode"]["hook:PreToolUse:audit"].clone();
    project
        .child("hooks/audit/plugin.ts")
        .write_str("export default 'replacement'\n")
        .unwrap();
    project
        .child(".opencode/opencode.json")
        .write_str("{}")
        .unwrap();

    let target_dir = project.child(".opencode");
    let original_permissions = fs::metadata(target_dir.path()).unwrap().permissions();
    fs::set_permissions(target_dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let attempted = sync(&project);
    fs::set_permissions(target_dir.path(), original_permissions).unwrap();
    attempted
        .success()
        .stderr(predicate::str::contains(
            "failed to remove prior hook entries",
        ))
        .stderr(predicate::str::contains("Permission denied"));

    let after: mars_agents::lock::LockFile =
        toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    assert_eq!(
        after.config_entries[".opencode"]["hook:PreToolUse:audit"],
        prior_record
    );
    plugin.assert(original.as_str());
    assert_config_entry_consistency(&project);
}
