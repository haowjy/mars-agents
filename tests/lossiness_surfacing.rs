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
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not());

    project.to_path_buf()
}

#[test]
fn sync_hides_lossiness_summary_without_verbose() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not());
}

#[test]
fn sync_json_has_no_lossiness_without_verbose() {
    let dir = TempDir::new().unwrap();
    let project = setup_lossy_synced_project(&dir);

    let output = mars()
        .args(["sync", "--json", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        json["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .all(|d| d["category"].as_str() != Some("lossiness")),
        "sync --json carries no launch-time lossiness by default: {stdout}"
    );
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

    offline_mars(dir.path())
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
fn check_is_silent_about_launch_time_fields_without_verbose() {
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
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not());
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
        .stderr(predicate::str::contains(LOSSINESS_VERBOSE_SNIPPET).not());
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

/// Sync an agent whose `tools` is `tools_yaml` to `.claude` and `.codex`; returns (stderr, verbose stderr).
fn sync_tools_agent(tools_yaml: &str) -> (String, String) {
    sync_tools_agent_in_project(tools_yaml, "")
}

/// As [`sync_tools_agent`], with `extra_toml` appended to the project's `mars.toml`.
fn sync_tools_agent_in_project(tools_yaml: &str, extra_toml: &str) -> (String, String) {
    let dir = TempDir::new().unwrap();
    let agent = format!("---\nname: a\ndescription: a\ntools:\n{tools_yaml}---\n# Body\n");
    let source = create_source(&dir, "base", &[("a", &agent)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\", \".codex\"]\nagent_emission = \"always\"\n\n[dependencies.base]\npath = \"{}\"\n{extra_toml}",
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    let run = |extra: &[&str]| {
        let out = mars()
            .arg("sync")
            .args(extra)
            .args(["--root", project.path().to_str().unwrap()])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stderr).unwrap()
    };
    (run(&[]), run(&["--verbose"]))
}

#[test]
fn scoped_bash_allow_is_not_a_lost_restriction() {
    let (quiet, verbose) = sync_tools_agent("  \"bash(rg *)\": allow\n");
    assert!(!quiet.contains("loses restrictions"), "{quiet}");
    assert!(!verbose.contains("only this"), "{verbose}");

    let (quiet, _) = sync_tools_agent("  \"bash(rg *)\": allow\n  bash: deny\n");
    assert!(
        quiet.contains("loses restrictions") && quiet.contains(".codex"),
        "{quiet}"
    );
}

#[test]
fn a_project_wide_deny_a_target_cannot_carry_is_one_project_loss() {
    // The profile's own `pkill` deny and the project's are one rule, lost once for the project.
    let (quiet, verbose) = sync_tools_agent_in_project(
        "  bash: allow\n  \"bash(pkill -f:*)\": deny\n",
        "\n[settings.tools]\n\"bash(pkill -f *)\" = \"deny\"\n",
    );
    assert!(
        quiet.contains("project setting `tools.bash(pkill -f *)`"),
        "{quiet}"
    );
    assert_eq!(
        verbose
            .matches("project `tools.bash(pkill -f *)`: 1 field dropped for .codex")
            .count(),
        1,
        "{verbose}"
    );
    assert!(
        !verbose.contains("agent `a`: 1 field dropped for .codex (tools (deny bash(pkill"),
        "the agent's own record no longer carries it: {verbose}"
    );
}
