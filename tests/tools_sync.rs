// qa-validated: mars-tools-abstraction

mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use std::fs;
use std::path::PathBuf;

use common::*;

fn setup_project_with_source(
    dir: &TempDir,
    targets: &[&str],
    agents: &[(&str, &str)],
) -> std::path::PathBuf {
    let source = create_source(dir, "base", agents, &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();

    let targets_str = targets
        .iter()
        .map(|target| format!("\"{target}\""))
        .collect::<Vec<_>>()
        .join(", ");

    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [{targets_str}]\nagent_emission = \"always\"\n\n[dependencies.base]\npath = \"{}\"\n",
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    project.to_path_buf()
}

fn scoped_tools_agent() -> (&'static str, &'static str) {
    (
        "scoped-tools-agent",
        "---\nname: scoped-tools-agent\ndescription: agent with scoped tools (dropped for claude)\nharness: claude\nmodel: claude-opus-4-6\ntools:\n  \"*\": deny\n  read:\n    \"*\": allow\n    \"*.env\": deny\n---\n# Scoped tools agent\n",
    )
}

#[test]
fn sync_abstract_tools_map_lowered_to_claude_disallowed_tools() {
    let dir = TempDir::new().unwrap();
    let project = setup_project_with_source(
        &dir,
        &[".claude"],
        &[(
            "restricted-agent",
            "---\nname: restricted-agent\ndescription: agent with tools restriction\nharness: claude\nmodel: claude-opus-4-6\ntools:\n  \"*\": allow\n  task: deny\n---\n# Restricted agent\n",
        )],
    );

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success();

    let canonical = fs::read_to_string(
        PathBuf::from(&project)
            .join(".mars")
            .join("agents")
            .join("restricted-agent.md"),
    )
    .unwrap();
    assert!(canonical.contains("task"));
    assert!(canonical.contains("deny"));

    let native = fs::read_to_string(
        PathBuf::from(&project)
            .join(".claude")
            .join("agents")
            .join("restricted-agent.md"),
    )
    .unwrap();
    assert!(native.contains("disallowed-tools"));
    assert!(native.contains("Agent"));
    assert!(!native.contains("task:"));
}

#[test]
fn sync_abstract_tools_map_infers_codex_sandbox_mode() {
    let dir = TempDir::new().unwrap();
    let project = setup_project_with_source(
        &dir,
        &[".codex"],
        &[(
            "workspace-coder",
            "---\nname: workspace-coder\ndescription: agent that can edit files\nharness: codex\nmodel: gpt-5.3-codex\ntools:\n  \"*\": deny\n  bash: allow\n  edit: allow\n---\n# Workspace coder\n",
        )],
    );

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success();

    let toml_str = fs::read_to_string(
        PathBuf::from(&project)
            .join(".codex")
            .join("agents")
            .join("workspace-coder.toml"),
    )
    .unwrap();
    let parsed: toml::Value = toml::from_str(&toml_str).unwrap();
    assert_eq!(
        parsed.get("sandbox_mode").and_then(|v| v.as_str()),
        Some("workspace-write"),
        "bash+edit allow with default deny should infer workspace-write"
    );
}

#[test]
fn sync_deprecated_tools_list_preserved_as_abstract_in_canonical() {
    let dir = TempDir::new().unwrap();
    let project = setup_project_with_source(
        &dir,
        &[".claude"],
        &[(
            "old-coder",
            "---\nname: old-coder\ndescription: uses old tools syntax\nharness: claude\nmodel: claude-opus-4-6\ntools: [Bash, Write, Edit]\n---\n# Old coder\n",
        )],
    );

    mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .assert()
        .success();

    let canonical = fs::read_to_string(
        PathBuf::from(&project)
            .join(".mars")
            .join("agents")
            .join("old-coder.md"),
    )
    .unwrap();
    assert!(
        canonical.contains("tools: [Bash, Write, Edit]"),
        "canonical artifact should preserve deprecated list form for now"
    );

    let native = fs::read_to_string(
        PathBuf::from(&project)
            .join(".claude")
            .join("agents")
            .join("old-coder.md"),
    )
    .unwrap();
    assert!(native.contains("Bash") || native.contains("bash"));
}

#[test]
fn sync_tools_lossiness_warns_on_sync_and_fails_strict_validation() {
    let dir = TempDir::new().unwrap();
    let project = setup_project_with_source(&dir, &[".claude"], &[scoped_tools_agent()]);

    let output = mars()
        .args(["sync", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "sync should succeed even with lossiness"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("agent-field-dropped") || stderr.contains("dropped"),
        "sync should warn about dropped scoped tools field: {stderr}"
    );

    mars()
        .args(["validate", "--strict", "--root", project.to_str().unwrap()])
        .assert()
        .failure();

    let output = mars()
        .args([
            "validate",
            "--json",
            "--strict",
            "--root",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert_eq!(json["clean"].as_bool(), Some(false));
    let diagnostics = json["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag["code"].as_str() == Some("agent-field-dropped")),
        "should have agent-field-dropped diagnostic: {json}"
    );
}
