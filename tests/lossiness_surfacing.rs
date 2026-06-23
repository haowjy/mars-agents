mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;

use common::*;

const LOSSY_CURSOR_AGENT: &str = r#"---
name: cursor-worker
description: Cursor worker
harness: cursor
harness-overrides:
  cursor:
    native-config:
      cursor.only: true
---
# Cursor body
"#;

const LOSSINESS_SNIPPET: &str = "not lowered (meridian-only) for .cursor (native-config)";

fn setup_lossy_synced_project(dir: &TempDir) -> std::path::PathBuf {
    let source = create_source(dir, "base", &[("cursor-worker", LOSSY_CURSOR_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".cursor"]
agent_emission = "always"

[dependencies.base]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SNIPPET));

    project.to_path_buf()
}

#[test]
fn sync_surfaces_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SNIPPET));
}

#[test]
fn validate_suppresses_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    let output = mars()
        .args(["validate", "--json", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        !diags.iter().any(|d| d["category"].as_str() == Some("lossiness")),
        "validate must not surface lossiness warnings: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains(LOSSINESS_SNIPPET),
        "validate stderr must not contain lossiness warning: {stderr}"
    );
}

#[test]
fn export_suppresses_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    let output = mars()
        .args(["export", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        !diags.iter().any(|d| d["category"].as_str() == Some("lossiness")),
        "export must not surface lossiness warnings: {stdout}"
    );
}

#[test]
fn add_suppresses_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let lossy_source = create_source(
        &dir,
        "lossy",
        &[("cursor-worker", LOSSY_CURSOR_AGENT)],
        &[],
    );
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(
            r#"[settings]
targets = [".cursor"]
agent_emission = "always"

[dependencies]
"#,
        )
        .unwrap();
    std::fs::create_dir_all(project.path().join(".mars")).unwrap();
    std::fs::create_dir_all(project.path().join(".cursor")).unwrap();

    mars()
        .args([
            "add",
            lossy_source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SNIPPET).not());
}

#[test]
fn check_surfaces_lossiness_for_configured_targets() {
    let dir = TempDir::new().unwrap();
    let pkg = create_source(&dir, "pkg", &[("cursor-worker", LOSSY_CURSOR_AGENT)], &[]);
    std::fs::write(
        pkg.join("mars.toml"),
        "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"always\"\n",
    )
    .unwrap();

    mars()
        .args(["check", pkg.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SNIPPET));
}
