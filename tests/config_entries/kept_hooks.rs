use super::support::*;

fn project_with_dependency(
    dir: &TempDir,
    targets: &str,
) -> (assert_fs::fixture::ChildPath, assert_fs::fixture::ChildPath) {
    let source = dir.child("source");
    source.create_dir_all().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [{targets}]\n[dependencies]\ntool = {{ path = \"{}\" }}\n",
            portable_path(source.path())
        ))
        .unwrap();
    (project, source)
}

/// A dependency hook for the merge target `.codex` and the file target `.opencode`.
fn write_guard(source: &assert_fs::fixture::ChildPath, manifest: &str) {
    write_hook(source, "guard", manifest);
    write_fragment(
        source,
        "guard",
        "codex.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf guard"}]}]}"#,
    );
    write_fragment(
        source,
        "guard",
        "plugin.ts",
        "export default { id: 'guard' }\n",
    );
}

const VALID: &str = "visibility = \"exported\"\n[targets.\".codex\"]\n[targets.\".opencode\"]\nfragment = \"plugin.ts\"\n";

fn snapshot(project: &assert_fs::fixture::ChildPath) -> (String, String, String) {
    (
        fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
        fs::read_to_string(project.child(".opencode/plugins/mars-guard.ts").path()).unwrap(),
        fs::read_to_string(project.child("mars.lock").path()).unwrap(),
    )
}

#[test]
fn dependency_hook_turned_invalid_keeps_its_install_until_fixed() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\", \".opencode\"");
    write_guard(&source, VALID);
    sync(&project).success();
    let installed = snapshot(&project);
    assert!(installed.0.contains("printf guard"));

    // `bogus` is an unknown key inside a target table: still invalid for a dependency.
    write_guard(
        &source,
        "visibility = \"exported\"\n[targets.\".codex\"]\nbogus = 1\n[targets.\".opencode\"]\nfragment = \"plugin.ts\"\n",
    );
    write_fragment(&source, "guard", "codex.json", "not json");
    for _ in 0..2 {
        sync(&project)
            .success()
            .stderr(predicate::str::contains("hook `guard`"))
            .stderr(predicate::str::contains(
                "keeping what the last sync installed",
            ));
        assert_eq!(snapshot(&project), installed);
        project
            .child(".codex/hooks/guard/codex.json")
            .assert(predicate::str::contains("printf guard"));
    }

    write_guard(&source, VALID);
    write_fragment(
        &source,
        "guard",
        "codex.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf fixed"}]}]}"#,
    );
    sync(&project)
        .success()
        .stderr(predicate::str::contains("keeping").not());
    let fixed = snapshot(&project);
    assert!(fixed.0.contains("printf fixed") && !fixed.0.contains("printf guard"));
}

#[test]
fn dependency_hook_never_installed_and_invalid_installs_nothing() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_guard(
        &source,
        "visibility = \"exported\"\n[targets.\".codex\"]\nbogus = 1\n",
    );
    sync(&project)
        .success()
        .stderr(predicate::str::contains("nothing is installed"));
    project
        .child(".codex/hooks.json")
        .assert(predicate::path::missing());
}

#[test]
fn dependency_unknown_key_and_event_warn_and_still_install() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_guard(
        &source,
        "visibility = \"exported\"\nfuture-key = true\n[targets.\".codex\"]\n",
    );
    write_fragment(
        &source,
        "guard",
        "codex.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf guard"}]}],"FutureEvent":[{"hooks":[]}]}"#,
    );
    sync(&project)
        .success()
        .stderr(predicate::str::contains("unknown key `future-key`"))
        .stderr(predicate::str::contains("unknown event `FutureEvent`"));
    project.child(".codex/hooks.json").assert(
        predicate::str::contains("printf guard").and(predicate::str::contains("FutureEvent").not()),
    );
}

#[test]
fn local_unknown_key_and_event_still_error() {
    for (manifest, fragment, message) in [
        (
            "future-key = true\n[targets.\".codex\"]\n",
            r#"{"SessionStart":[]}"#,
            "future-key",
        ),
        (
            "[targets.\".codex\"]\n",
            r#"{"FutureEvent":[]}"#,
            "unknown event `FutureEvent`",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str("[settings]\ntargets = [\".codex\"]\n")
            .unwrap();
        write_hook(&project, "guard", manifest);
        write_fragment(&project, "guard", "codex.json", fragment);
        sync(&project)
            .failure()
            .stderr(predicate::str::contains(message));
    }
}

#[test]
fn resync_without_changes_leaves_the_merge_file_untouched() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_guard(&source, "visibility = \"exported\"\n[targets.\".codex\"]\n");
    sync(&project).success();
    let path = project.child(".codex/hooks.json");
    let before = fs::metadata(path.path()).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    sync(&project).success();
    assert_eq!(
        fs::metadata(path.path()).unwrap().modified().unwrap(),
        before
    );
}
