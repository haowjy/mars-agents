//! `[hooks]` in `hook.toml`: selection, `skip-targets`, per-event override, reports, lock
//! records and removal.

use super::support::*;

const GUARD: &str = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = 'bash "${MARS_HOOK_DIR}/run.sh" ${MARS_HOOK_TARGET}'
timeout = 5.5

[[hooks.Notification]]
[[hooks.Notification.hooks]]
type = "command"
command = "echo n"
"#;

fn project(dir: &TempDir, targets: &str) -> assert_fs::fixture::ChildPath {
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!("[settings]\ntargets = [{targets}]\n"))
        .unwrap();
    project
}

fn json(project: &assert_fs::fixture::ChildPath, path: &str) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(project.child(path).path()).unwrap()).unwrap()
}

fn lock_keys(project: &assert_fs::fixture::ChildPath, target: &str) -> Vec<String> {
    let lock: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    lock.get("config_entries")
        .and_then(|entries| entries.get(target))
        .and_then(Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

fn commands(settings: &serde_json::Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["hooks"].as_array().unwrap())
        .map(|handler| handler["command"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn hooks_render_for_claude_and_codex_and_report_the_rest() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".claude\", \".codex\", \".opencode\"");
    write_hook(&project, "guard", GUARD);

    sync(&project)
        .success()
        .stderr(predicate::str::contains(
            "`.codex` does not run `Notification`: Codex has no such event",
        ))
        .stderr(predicate::str::contains(
            "`.opencode` does not run `PreToolUse`: mars does not render hooks for OpenCode yet",
        ));

    let claude = json(&project, ".claude/settings.local.json");
    let codex = json(&project, ".codex/hooks.json");
    let root = portable_path(project.path());
    // Each target gets its own directory in the command, and its own target token.
    assert_eq!(
        commands(&claude, "PreToolUse"),
        [format!(
            "bash \"{root}/.claude/hooks/guard/run.sh\" .claude"
        )]
    );
    assert_eq!(
        commands(&codex, "PreToolUse"),
        [format!("bash \"{root}/.codex/hooks/guard/run.sh\" .codex")]
    );
    // Claude keeps the fractional timeout; Codex gets whole seconds.
    assert_eq!(claude["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 5.5);
    assert_eq!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 6);
    assert_eq!(commands(&claude, "Notification"), ["echo n"]);
    assert!(codex["hooks"].get("Notification").is_none());

    assert_eq!(
        lock_keys(&project, ".claude"),
        ["hook:Notification:guard", "hook:PreToolUse:guard"]
    );
    assert_eq!(lock_keys(&project, ".codex"), ["hook:PreToolUse:guard"]);
    // OpenCode renders nothing yet, so it gets neither a copy of the directory nor a record.
    project
        .child(".opencode/hooks")
        .assert(predicate::path::missing());
    assert!(lock_keys(&project, ".opencode").is_empty());
}

#[test]
fn skip_targets_turns_generated_entries_off_for_that_target() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".claude\", \".codex\"");
    write_hook(
        &project,
        "guard",
        &format!("skip-targets = [\".codex\"]\n{GUARD}"),
    );

    sync(&project)
        .success()
        // A skipped target gets no skip reports.
        .stderr(predicate::str::contains("`.codex` does not run").not());
    project
        .child(".codex/hooks.json")
        .assert(predicate::path::missing());
    project
        .child(".codex/hooks/guard")
        .assert(predicate::path::missing());
    assert!(project.child(".claude/settings.local.json").exists());
}

#[test]
fn a_fragment_replaces_only_its_events_and_an_empty_array_turns_one_off() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".claude\", \".codex\"");
    write_hook(
        &project,
        "guard",
        &format!("{GUARD}\n[targets.\".codex\"]\nfragment = \"codex.json\"\n"),
    );
    write_fragment(
        &project,
        "guard",
        "codex.json",
        r#"{"PreToolUse":[],"Stop":[{"hooks":[{"type":"command","command":"printf stop"}]}]}"#,
    );

    sync(&project)
        .success()
        .stderr(
            predicate::str::contains("hook-fragment-overrides").or(predicate::str::contains(
                "replaces the generated `PreToolUse` entries",
            )),
        );

    let codex = json(&project, ".codex/hooks.json");
    // PreToolUse is off on Codex; Stop is the fragment's; Claude still has the guard.
    assert!(codex["hooks"].get("PreToolUse").is_none());
    assert_eq!(commands(&codex, "Stop"), ["printf stop"]);
    assert_eq!(lock_keys(&project, ".codex"), ["hook:Stop:guard"]);
    let claude = json(&project, ".claude/settings.local.json");
    assert_eq!(commands(&claude, "PreToolUse").len(), 1);
}

#[test]
fn removing_the_hook_removes_its_generated_entries_and_records() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".claude\", \".codex\"");
    write_hook(&project, "guard", GUARD);
    sync(&project).success();
    // A user's own entry sits beside the generated ones.
    let path = project.child(".codex/hooks.json");
    let mut codex = json(&project, ".codex/hooks.json");
    codex["hooks"]["Stop"] =
        serde_json::json!([{"hooks": [{"type": "command", "command": "mine"}]}]);
    path.write_str(&serde_json::to_string_pretty(&codex).unwrap())
        .unwrap();

    fs::remove_dir_all(project.child("hooks/guard").path()).unwrap();
    sync(&project).success();

    let codex = json(&project, ".codex/hooks.json");
    assert!(codex["hooks"].get("PreToolUse").is_none());
    assert_eq!(commands(&codex, "Stop"), ["mine"]);
    let claude = json(&project, ".claude/settings.local.json");
    assert!(claude["hooks"].get("PreToolUse").is_none());
    assert!(lock_keys(&project, ".claude").is_empty());
    assert!(lock_keys(&project, ".codex").is_empty());
    project
        .child(".claude/hooks/guard")
        .assert(predicate::path::missing());
}

#[test]
fn codex_trust_is_reported_when_handler_positions_change_and_not_otherwise() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".codex\"");
    write_hook(&project, "guard", GUARD);
    sync(&project).success().stderr(
        predicate::str::contains("codex-hooks-need-trust")
            .or(predicate::str::contains("skips them until you review them")),
    );
    // Nothing changed: nothing to review again.
    sync(&project)
        .success()
        .stderr(predicate::str::contains("skips them until you review them").not());

    // A second hook that sorts ahead moves the first hook's position? It lands after it
    // within the event, so only the new handler is new.
    write_hook(
        &project,
        "audit",
        "order = -1\n[[hooks.PreToolUse]]\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"echo audit\"\n",
    );
    sync(&project)
        .success()
        .stderr(predicate::str::contains("skips them until you review them"));
    let codex = json(&project, ".codex/hooks.json");
    assert_eq!(commands(&codex, "PreToolUse").len(), 2);
}

#[test]
fn a_dependency_hook_with_an_unknown_event_installs_its_other_events() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("source");
    source.create_dir_all().unwrap();
    let project = project(&dir, "\".claude\", \".codex\"");
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\", \".codex\"]\n[dependencies]\ntool = {{ path = \"{}\" }}\n",
            portable_path(source.path())
        ))
        .unwrap();
    write_hook(
        &source,
        "guard",
        &format!(
            "visibility = \"exported\"\n{GUARD}\n[[hooks.FutureEvent]]\n[[hooks.FutureEvent.hooks]]\ntype = \"command\"\ncommand = \"echo f\"\n"
        ),
    );

    sync(&project)
        .success()
        .stderr(predicate::str::contains("unknown event `FutureEvent`"));
    let claude = json(&project, ".claude/settings.local.json");
    assert_eq!(commands(&claude, "PreToolUse").len(), 1);
    assert!(claude["hooks"].get("FutureEvent").is_none());
    assert_eq!(lock_keys(&project, ".codex"), ["hook:PreToolUse:guard"]);
}

#[test]
fn the_projects_own_unknown_event_is_an_error_unless_unchecked() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".claude\", \".codex\"");
    let future = "[[hooks.FutureEvent]]\n[[hooks.FutureEvent.hooks]]\ntype = \"command\"\ncommand = \"echo f\"\n";
    write_hook(&project, "guard", &format!("{GUARD}\n{future}"));
    sync(&project)
        .failure()
        .stderr(predicate::str::contains("unknown event `FutureEvent`"));
    project
        .child(".claude/settings.local.json")
        .assert(predicate::path::missing());

    write_hook(
        &project,
        "guard",
        &format!("unchecked = true\n{GUARD}\n{future}"),
    );
    sync(&project).success().stderr(predicate::str::contains(
        "`.codex` does not run `FutureEvent`",
    ));
    // Claude only.
    let claude = json(&project, ".claude/settings.local.json");
    assert_eq!(commands(&claude, "FutureEvent"), ["echo f"]);
    assert!(
        json(&project, ".codex/hooks.json")["hooks"]
            .get("FutureEvent")
            .is_none()
    );
}

#[test]
fn a_target_table_beside_hooks_must_name_its_fragment() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "\".codex\"");
    write_hook(
        &project,
        "guard",
        &format!("{GUARD}\n[targets.\".codex\"]\n"),
    );
    write_fragment(&project, "guard", "codex.json", r#"{"Stop":[]}"#);
    sync(&project)
        .failure()
        .stderr(predicate::str::contains("must name its `fragment`"));
}

#[test]
fn check_warns_about_the_floor_and_a_condition_that_never_runs() {
    let dir = TempDir::new().unwrap();
    let package = dir.child("package");
    package.create_dir_all().unwrap();
    package
        .child("mars.toml")
        .write_str("[package]\nname = \"pkg\"\nversion = \"1.0.0\"\n")
        .unwrap();
    write_hook(
        &package,
        "guard",
        &format!(
            "{GUARD}\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"x\"\nif = \"Bash(*)\"\n"
        ),
    );
    mars()
        .args(["check", package.path().to_str().unwrap()])
        .assert()
        .stdout(predicate::str::contains(
            "`requires-mars` should be `\">=0.25.0\"`",
        ))
        .stdout(predicate::str::contains("`if` on `Stop` never matches"))
        .stdout(predicate::str::contains(
            "`.codex` does not run `Notification`",
        ));
}
