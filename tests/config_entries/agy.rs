use super::support::*;

/// A project that manages `.agy`, with one package skill so the sync has something to install.
fn agy_project(dir: &TempDir) -> assert_fs::fixture::ChildPath {
    let source = create_source(
        dir,
        "src",
        &[],
        &[("plain", "---\nname: plain\ndescription: d\n---\n")],
    );
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".agy\"]\n\n[dependencies.src]\npath = \"{}\"\n",
            portable_path(&source)
        ))
        .unwrap();
    project
}

fn json(project: &assert_fs::fixture::ChildPath, file: &str) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(project.child(file).path()).unwrap()).unwrap()
}

fn stderr(assertion: &assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assertion.get_output().stderr).into_owned()
}

#[test]
fn sync_writes_the_pointer_and_manifest_keys() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    project
        .child(".agents/plugins.json")
        .write_str(r#"{"entries":[{"path":"user-plugin","enabled":true}]}"#)
        .unwrap();

    sync(&project).success();

    // The array section keeps the user's entry first and appends Mars's.
    assert_eq!(
        json(&project, ".agents/plugins.json"),
        serde_json::json!({"entries": [
            {"path": "user-plugin", "enabled": true},
            {"path": ".agy"},
        ]})
    );
    // The top-level section is named after the project directory.
    assert_eq!(
        json(&project, ".agy/plugin.json"),
        serde_json::json!({"name": "mars-project", "version": "1.0.0"})
    );
}

#[test]
fn an_identical_pointer_is_adopted_without_a_rewrite() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    let original = "{\"entries\": [{\"path\": \".agy\"}]}";
    project
        .child(".agents/plugins.json")
        .write_str(original)
        .unwrap();

    let assertion = sync(&project).success();

    assert!(
        !stderr(&assertion).contains("plugin-pointer"),
        "{}",
        stderr(&assertion)
    );
    assert_eq!(
        fs::read_to_string(project.child(".agents/plugins.json").path()).unwrap(),
        original
    );
}

#[test]
fn an_edited_manifest_key_is_kept_and_warned_until_forced() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    sync(&project).success();
    project
        .child(".agy/plugin.json")
        .write_str(r#"{"name":"mine","version":"1.0.0"}"#)
        .unwrap();

    let assertion = sync(&project).success();
    assert!(
        stderr(&assertion).contains("plugin manifest key `name`"),
        "{}",
        stderr(&assertion)
    );
    assert_eq!(json(&project, ".agy/plugin.json")["name"], "mine");

    sync_force(&project).success();
    assert_eq!(json(&project, ".agy/plugin.json")["name"], "mars-project");
}

#[test]
fn a_manifest_key_mars_did_not_write_collides() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    project
        .child(".agy/plugin.json")
        .write_str(r#"{"name":"theirs","description":"kept"}"#)
        .unwrap();

    let assertion = sync(&project).success();

    assert!(
        stderr(&assertion).contains("exists but mars does not own it"),
        "{}",
        stderr(&assertion)
    );
    let manifest = json(&project, ".agy/plugin.json");
    assert_eq!(manifest["name"], "theirs");
    assert_eq!(manifest["description"], "kept");
    // The key beside the collision is still Mars's to write.
    assert_eq!(manifest["version"], "1.0.0");
}

#[test]
fn unlink_removes_only_marss_entries_and_keeps_the_rest_in_order() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    project
        .child(".agents/plugins.json")
        .write_str(r#"{"entries":[{"path":"a"},{"path":"b"}]}"#)
        .unwrap();
    sync(&project).success();

    mars()
        .args(["unlink", ".agy", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    assert_eq!(
        json(&project, ".agents/plugins.json"),
        serde_json::json!({"entries": [{"path": "a"}, {"path": "b"}]})
    );
    assert!(!project.child(".agy").exists());
}

#[test]
fn an_invalid_plugin_list_fails_the_sync_before_anything_is_written() {
    let dir = TempDir::new().unwrap();
    let project = agy_project(&dir);
    project
        .child(".agents/plugins.json")
        .write_str(r#"{"entries": 3}"#)
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .failure();

    assert!(!project.child(".agy/plugin.json").exists());
    assert_eq!(
        fs::read_to_string(project.child(".agents/plugins.json").path()).unwrap(),
        r#"{"entries": 3}"#
    );
}
