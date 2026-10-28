mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;

use common::*;

const LOSSY_CURSOR_AGENT: &str = r#"---
name: cursor-worker
description: Cursor worker
harness: cursor
model-policies:
  - match: {model: gpt-6}
    override: {effort: low}
---
# Cursor body
"#;

const LOSSINESS_SUMMARY_SNIPPET: &str = "launch-time field mapping handled by meridian at spawn";
const LOSSINESS_VERBOSE_SNIPPET: &str = "not lowered (meridian-only) for .cursor (model-policies)";

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
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET));

    project.to_path_buf()
}

#[test]
fn validate_and_export_suppress_lossiness_warnings() {
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
        !diags
            .iter()
            .any(|d| d["category"].as_str() == Some("lossiness")),
        "validate must not surface lossiness warnings: {stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains(LOSSINESS_VERBOSE_SNIPPET),
        "validate stderr must not contain lossiness warning: {stderr}"
    );
    assert!(
        !stderr.contains(LOSSINESS_SUMMARY_SNIPPET),
        "validate stderr must not contain lossiness summary: {stderr}"
    );

    let output = mars()
        .args(["export", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        !diags
            .iter()
            .any(|d| d["category"].as_str() == Some("lossiness")),
        "export must not surface lossiness warnings: {stdout}"
    );
}

#[test]
fn add_suppresses_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let lossy_source = create_source(&dir, "lossy", &[("cursor-worker", LOSSY_CURSOR_AGENT)], &[]);
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

    offline_mars(dir.path())
        .args([
            "add",
            lossy_source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not())
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET).not());
}

#[test]
fn check_surfaces_lossiness_summary_and_verbose_detail() {
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
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET));

    mars()
        .args(["check", "--verbose", pkg.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET));
}

#[test]
fn check_skips_agent_lossiness_when_emission_suppressed() {
    let dir = TempDir::new().unwrap();
    let pkg = create_source(&dir, "pkg", &[("cursor-worker", LOSSY_CURSOR_AGENT)], &[]);
    std::fs::write(
        pkg.join("mars.toml"),
        "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"never\"\n",
    )
    .unwrap();

    mars()
        .args(["check", pkg.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET).not());
}

#[test]
fn sync_verbose_surfaces_meridian_only_detail() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    mars()
        .args(["sync", "--verbose", "--root", project.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET));
}
