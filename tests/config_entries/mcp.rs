use super::support::*;

fn create_dependency_source(
    dir: &TempDir,
    name: &str,
    dependency: Option<(&str, &std::path::Path)>,
) -> std::path::PathBuf {
    let source = dir.child(name);
    source.create_dir_all().unwrap();
    let mut manifest = format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n");
    if let Some((dependency_name, dependency_path)) = dependency {
        manifest.push_str(&format!(
            "\n[dependencies.{dependency_name}]\npath = \"{}\"\n",
            portable_path(dependency_path)
        ));
    }
    source.child("mars.toml").write_str(&manifest).unwrap();
    source.to_path_buf()
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
    assert_config_entry_consistency(&project);

    let mcp_path = project.child(".claude").child(".mcp.json");
    let installed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(mcp_path.path()).unwrap()).unwrap();
    assert!(installed["mcpServers"]["context7"].is_object());

    mars()
        .args(["remove", "base", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    assert_config_entry_consistency(&project);

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
fn direct_override_preserves_matching_transitive_declared_identity() {
    let dir = TempDir::new().unwrap();
    let original = create_dependency_source(&dir, "original", None);
    let replacement = create_dependency_source(&dir, "replacement", None);
    let workflow = create_dependency_source(&dir, "workflow", Some(("shared", original.as_path())));
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.shared]\npath = \"{}\"\n\n\
             [dependencies.workflow]\npath = \"{}\"\n",
            portable_path(&original),
            portable_path(&workflow),
        ))
        .unwrap();

    mars()
        .args([
            "override",
            "shared",
            "--path",
            replacement.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let local = fs::read_to_string(project.child("mars.local.toml").path()).unwrap();
    assert!(local.contains("[overrides.shared]"));
    assert!(local.contains("replacement"));
}

#[test]
fn direct_override_does_not_collapse_conflicting_transitive_declared_identity() {
    let dir = TempDir::new().unwrap();
    let direct_original = create_dependency_source(&dir, "direct-original", None);
    let transitive_original = create_dependency_source(&dir, "transitive-original", None);
    let replacement = create_dependency_source(&dir, "replacement", None);
    let workflow = create_dependency_source(
        &dir,
        "workflow",
        Some(("shared", transitive_original.as_path())),
    );
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.shared]\npath = \"{}\"\n\n\
             [dependencies.workflow]\npath = \"{}\"\n",
            portable_path(&direct_original),
            portable_path(&workflow),
        ))
        .unwrap();

    mars()
        .args([
            "override",
            "shared",
            "--path",
            replacement.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("conflicting identities"))
        .stderr(predicate::str::contains("direct-original"))
        .stderr(predicate::str::contains("transitive-original"));

    project
        .child("mars.local.toml")
        .assert(predicate::path::missing());
}

#[test]
fn unused_override_diagnostic_remains_emitted() {
    let dir = TempDir::new().unwrap();
    let source = create_dependency_source(&dir, "base", None);
    let replacement = create_dependency_source(&dir, "replacement", None);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n",
            portable_path(&source)
        ))
        .unwrap();
    project
        .child("mars.local.toml")
        .write_str(&format!(
            "[overrides.unused]\npath = \"{}\"\n",
            portable_path(&replacement)
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "override `unused` references a dependency not in the resolved project graph",
        ));
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
        .stdout(predicate::str::contains("unlinked `.claude`"));

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
        .stdout(predicate::str::contains("unlinked `.agents`"));

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
fn unlink_removes_only_owned_outputs_and_preserves_unmanaged_siblings() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();

    mars()
        .args([
            "init",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\nagent_emission = \"always\"\n")
        .unwrap();

    project
        .child(".mars-src")
        .child("agents")
        .create_dir_all()
        .unwrap();
    project
        .child(".mars-src")
        .child("agents")
        .child("owned.md")
        .write_str("# Owned")
        .unwrap();
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let unmanaged = project.child(".claude").child("agents").child("keep.md");
    unmanaged.write_str("# Handwritten").unwrap();

    mars()
        .args([
            "unlink",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "unlinked `.claude` (removed 1 managed output)",
        ));

    assert!(
        !project
            .child(".claude")
            .child("agents")
            .child("owned.md")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(unmanaged.path()).unwrap(),
        "# Handwritten"
    );
    assert!(project.child(".claude").exists());
}

#[test]
fn unlink_retires_pending_deletion_output_and_counts_it() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();

    mars()
        .args([
            "init",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\nagent_emission = \"always\"\n")
        .unwrap();
    project.child(".mars-src/agents").create_dir_all().unwrap();
    project
        .child(".mars-src/agents/owned.md")
        .write_str("# Owned")
        .unwrap();
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let mut lock = mars_agents::lock::load(project.path()).unwrap();
    let pending = lock
        .items
        .values_mut()
        .flat_map(|item| &mut item.outputs)
        .find(|output| {
            output.target_root == ".claude" && output.dest_path.as_str() == "agents/owned.md"
        })
        .expect("native output");
    pending.mark_pending_deletion();
    mars_agents::lock::write(project.path(), &lock).unwrap();

    mars()
        .args([
            "unlink",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "unlinked `.claude` (removed 1 managed output)",
        ));

    project
        .child(".claude/agents/owned.md")
        .assert(predicate::path::missing());
    let lock = mars_agents::lock::load(project.path()).unwrap();
    assert!(
        !lock.contains_output(".claude", "agents/owned.md"),
        "unlink must retire confirmed deletion authority"
    );
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
