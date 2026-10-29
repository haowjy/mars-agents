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

/// A dependency hook `hooks/protect` that installs as `guard` for `.codex`.
fn write_renamed_guard(source: &assert_fs::fixture::ChildPath, manifest: &str) {
    write_hook(source, "protect", manifest);
    write_fragment(
        source,
        "protect",
        "codex.json",
        r#"{"SessionStart":[{"hooks":[{"type":"command","command":"printf guard"}]}]}"#,
    );
}

const RENAMED: &str = "name = \"guard\"\nvisibility = \"exported\"\n[targets.\".codex\"]\n";

#[test]
fn a_renamed_dependency_hook_stays_kept_when_its_toml_stops_parsing() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_renamed_guard(&source, RENAMED);
    sync(&project).success();
    let installed = fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap();
    assert!(installed.contains("printf guard"));

    write_renamed_guard(&source, &format!("{RENAMED}[[[broken\n"));
    for _ in 0..2 {
        sync(&project)
            .success()
            .stderr(predicate::str::contains(
                "keeping what the last sync installed",
            ))
            .stderr(predicate::str::contains("never installed").not());
        assert_eq!(
            fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
            installed
        );
    }
}

#[test]
fn a_dependency_fragment_that_stops_loading_keeps_the_install_and_names_the_package() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_guard(
        &source,
        VALID
            .replace("[targets.\".opencode\"]\nfragment = \"plugin.ts\"\n", "")
            .as_str(),
    );
    sync(&project).success();
    let installed = fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap();

    // A fragment of the wrong shape, then one that is gone.
    for broken in [Some(r#"{"Stop": {}}"#), None] {
        match broken {
            Some(text) => write_fragment(&source, "guard", "codex.json", text),
            None => fs::remove_file(source.child("hooks/guard/codex.json").path()).unwrap(),
        }
        sync(&project)
            .success()
            .stderr(predicate::str::contains(
                "source package `tool`, hook `guard`, `codex.json`",
            ))
            .stderr(predicate::str::contains(
                "keeping what the last sync installed",
            ))
            .stderr(predicate::str::contains(".staging").not());
        assert_eq!(
            fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap(),
            installed
        );
    }
}

#[test]
fn the_projects_own_fragment_that_does_not_load_still_fails_the_sync() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".codex\"]\n")
        .unwrap();
    write_hook(&project, "mine", "[targets.\".codex\"]\n");
    write_fragment(&project, "mine", "codex.json", r#"{"Stop": {}}"#);
    sync(&project)
        .failure()
        .stderr(predicate::str::contains("must be an array"));
}

#[test]
fn an_invalid_dependency_hook_the_consumer_excludes_is_removed_and_not_reported() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    write_guard(&source, "visibility = \"exported\"\n[targets.\".codex\"]\n");
    sync(&project).success();
    assert!(
        fs::read_to_string(project.child(".codex/hooks.json").path())
            .unwrap()
            .contains("printf guard")
    );

    write_guard(
        &source,
        "visibility = \"exported\"\n[targets.\".codex\"]\nbogus = 1\n",
    );
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".codex\"]\n[dependencies]\ntool = {{ path = \"{}\", exclude = [\"guard\"] }}\n",
            portable_path(source.path())
        ))
        .unwrap();
    sync(&project)
        .success()
        .stderr(predicate::str::contains("hook-invalid-kept").not())
        .stderr(predicate::str::contains("keeping").not());
    assert!(
        !fs::read_to_string(project.child(".codex/hooks.json").path())
            .map(|text| text.contains("printf guard"))
            .unwrap_or(false)
    );
    project
        .child(".codex/hooks/guard")
        .assert(predicate::path::missing());

    // A hook the dependency does not export is never the consumer's to hear about.
    write_hook(&source, "internal", "[targets.\".codex\"]\nbogus = 1\n");
    sync(&project)
        .success()
        .stderr(predicate::str::contains("internal").not());
}

#[test]
fn a_kept_hook_keeps_its_place_among_the_merged_hooks() {
    let dir = TempDir::new().unwrap();
    let (project, source) = project_with_dependency(&dir, "\".codex\"");
    let write = |name: &str, bogus: &str| {
        write_hook(
            &source,
            name,
            &format!("visibility = \"exported\"\n[targets.\".codex\"]\n{bogus}"),
        );
        write_fragment(
            &source,
            name,
            "codex.json",
            &format!(
                r#"{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"echo {name}"}}]}}]}}"#
            ),
        );
    };
    let order = || {
        let text = fs::read_to_string(project.child(".codex/hooks.json").path()).unwrap();
        (text.find("echo a").unwrap(), text.find("echo b").unwrap())
    };
    write("a", "");
    write("b", "");
    sync(&project).success();
    let (a, b) = order();
    assert!(a < b);

    // Codex keys its trust on position: breaking and fixing `a` must move nothing.
    write("a", "bogus = 1\n");
    sync(&project)
        .success()
        .stderr(predicate::str::contains(
            "keeping what the last sync installed",
        ))
        .stderr(predicate::str::contains("codex-hooks-need-trust").not())
        .stderr(predicate::str::contains("moved").not());
    let (a, b) = order();
    assert!(a < b, "the kept hook stays first");

    write("a", "");
    sync(&project)
        .success()
        .stderr(predicate::str::contains("moved").not());
    let (a, b) = order();
    assert!(a < b);
}
