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

const LOSSINESS_SUMMARY_SNIPPET: &str = "launch-time field mapping handled by meridian at spawn";
const LOSSINESS_VERBOSE_SNIPPET: &str = "not lowered (meridian-only) for .cursor (native-config)";

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
fn sync_surfaces_lossiness_warnings() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET));
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

    mars()
        .args([
            "add",
            lossy_source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not());
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
        .stderr(predicate::str::contains(LOSSINESS_SUMMARY_SNIPPET));
}

#[test]
fn check_and_sync_agree_on_foreign_claude_skill_lossiness() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("claude-skill");
    let skill = source.child("skills/demo");
    skill.create_dir_all().unwrap();
    skill
        .child("SKILL.md")
        .write_str("---\nname: demo\ndescription: d\nmodel-invocable: false\ntools: [Bash(git *)]\n---\n# Body\n")
        .unwrap();
    source
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n")
        .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n\n[dependencies.base]\npath = \"{}\"\n",
            source.path().display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let sync_stderr = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(sync_stderr.status.success());
    let sync_stderr_text = String::from_utf8_lossy(&sync_stderr.stderr);
    let sync_lossiness: Vec<_> = sync_stderr_text
        .lines()
        .filter(|line| line.contains("field dropped") && line.contains(".codex"))
        .collect();
    assert!(
        !sync_lossiness.is_empty(),
        "sync should warn about lifted skill lossiness: {sync_stderr_text}"
    );

    let check_stderr = mars()
        .args(["check", source.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(check_stderr.status.success());
    let check_stderr_text = String::from_utf8_lossy(&check_stderr.stderr);
    let check_lossiness: Vec<_> = check_stderr_text
        .lines()
        .filter(|line| line.contains("field dropped") && line.contains(".codex"))
        .collect();
    assert_eq!(
        sync_lossiness, check_lossiness,
        "check must match sync lossiness for foreign claude skill"
    );
}

#[test]
fn check_respects_skill_variant_selection_like_sync() {
    let dir = TempDir::new().unwrap();
    let source = dir.child("variant-skill");
    let skill = source.child("skills/demo");
    skill.create_dir_all().unwrap();
    skill.child("variants/codex").create_dir_all().unwrap();
    skill
        .child("SKILL.md")
        .write_str("---\nname: demo\ndescription: d\nwhen_to_use: planning\n---\n# Base body\n")
        .unwrap();
    skill
        .child("variants/codex/SKILL.md")
        .write_str("# Codex-only body\n")
        .unwrap();
    source
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n")
        .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".codex\"]\nagent_emission = \"always\"\n\n[dependencies.base]\npath = \"{}\"\n",
            source.path().display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let sync_out = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(sync_out.status.success());
    let sync_lossiness: Vec<String> = String::from_utf8_lossy(&sync_out.stderr)
        .lines()
        .filter(|line| line.contains("lossiness") || line.contains("when_to_use"))
        .map(str::to_string)
        .collect();

    let check_out = mars()
        .args(["check", source.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(check_out.status.success());
    let check_lossiness: Vec<String> = String::from_utf8_lossy(&check_out.stderr)
        .lines()
        .filter(|line| line.contains("lossiness") || line.contains("when_to_use"))
        .map(str::to_string)
        .collect();

    assert_eq!(
        sync_lossiness, check_lossiness,
        "check must match sync skill variant lossiness"
    );
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

#[test]
fn check_verbose_surfaces_meridian_only_detail() {
    let dir = TempDir::new().unwrap();
    let pkg = create_source(&dir, "pkg", &[("cursor-worker", LOSSY_CURSOR_AGENT)], &[]);
    std::fs::write(
        pkg.join("mars.toml"),
        "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"always\"\n",
    )
    .unwrap();

    mars()
        .args(["check", "--verbose", pkg.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET));
}
