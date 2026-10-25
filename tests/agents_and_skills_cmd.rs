//! Integration tests for `mars agents` and `mars skills` CLI commands.
//!
//! Pins the `--json` output contract:
//!
//! - List envelope shapes: `{"agents":[...]}` and `{"skills":[...]}`
//! - Kebab-case `model-invocable` on both skills list and skills show.
//!   Regression guard: list previously emitted `model_invocable` (snake)
//!   while show emitted `model-invocable` (kebab).
//! - `subagents` array present in agent show output
//! - Not-found exits non-zero

mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;

use common::*;

const AGENT_CONTENT: &str = "---
name: orchestrator
description: Orchestrates work
mode: primary
subagents:
  - worker
---
# Orchestrator
";

const SKILL_CONTENT: &str = "---
name: planning
description: Planning helper
metadata:
  type: principle
model-invocable: false
---
# Planning
";

const FALLBACK_AGENT_CONTENT: &str = "---
name: worker
description: Fallback worker
mode: subagent
---
# Worker
";

const FALLBACK_SKILL_CONTENT: &str = "---
name: review
description: Fallback review helper
metadata:
  type: guardrail
---
# Review
";

// ── mars agents --json ────────────────────────────────────────────────────────

#[test]
fn agents_list_json_envelope_and_fields() {
    let dir = TempDir::new().unwrap();
    let project =
        setup_synced_project(&dir, "proj", "src", &[("orchestrator", AGENT_CONTENT)], &[]);

    let output = mars()
        .args(["--json", "agents", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success(), "mars agents should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("mars agents --json must be valid JSON:\n{stdout}"));

    let agents = json["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("expected top-level 'agents' array in JSON:\n{stdout}"));
    assert!(!agents.is_empty(), "expected at least one agent:\n{stdout}");

    let entry = agents
        .iter()
        .find(|e| e["name"] == "orchestrator")
        .unwrap_or_else(|| panic!("expected 'orchestrator' in agents list:\n{stdout}"));

    assert!(entry["name"].is_string(), "'name' key required:\n{stdout}");
    assert!(
        entry["description"].is_string(),
        "'description' key required:\n{stdout}"
    );
    assert!(entry["mode"].is_string(), "'mode' key required:\n{stdout}");
    assert_eq!(entry["mode"], "primary", "'mode' value:\n{stdout}");
}

#[test]
fn agents_show_json_subagents_and_kebab_keys() {
    let dir = TempDir::new().unwrap();
    let project =
        setup_synced_project(&dir, "proj", "src", &[("orchestrator", AGENT_CONTENT)], &[]);

    let output = mars()
        .args([
            "--json",
            "agents",
            "show",
            "orchestrator",
            "--root",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "mars agents show should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("mars agents show --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["name"], "orchestrator", "'name' value:\n{stdout}");
    assert!(
        json["description"].is_string(),
        "'description' required:\n{stdout}"
    );
    assert!(
        json["skills"].is_array(),
        "'skills' array required:\n{stdout}"
    );
    assert!(
        json["subagents"].is_array(),
        "'subagents' array required:\n{stdout}"
    );
    let subagents = json["subagents"].as_array().unwrap();
    assert_eq!(
        subagents,
        &[serde_json::json!("worker")],
        "'subagents' list:\n{stdout}"
    );

    // Tool policy is one `rules` list, in the launch bundle's rule shape.
}

#[test]
fn agents_show_not_found_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let project =
        setup_synced_project(&dir, "proj", "src", &[("orchestrator", AGENT_CONTENT)], &[]);

    let status = mars()
        .args([
            "--json",
            "agents",
            "show",
            "nonexistent",
            "--root",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap()
        .status;

    assert!(
        !status.success(),
        "agents show of missing agent must exit non-zero"
    );
}

#[test]
fn fallback_root_layers_agent_and_skill_lists_with_scope() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "project-src",
        &[("orchestrator", AGENT_CONTENT)],
        &[("planning", SKILL_CONTENT)],
    );
    let fallback = setup_synced_project(
        &dir,
        "fallback",
        "fallback-src",
        &[("worker", FALLBACK_AGENT_CONTENT)],
        &[("review", FALLBACK_SKILL_CONTENT)],
    );

    let agents_output = mars()
        .args([
            "--json",
            "agents",
            "list",
            "--root",
            project.to_str().unwrap(),
            "--fallback-root",
            fallback.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(agents_output.status.success());
    let agents: serde_json::Value = serde_json::from_slice(&agents_output.stdout).unwrap();
    let agents = agents["agents"].as_array().unwrap();
    assert_eq!(
        agents
            .iter()
            .find(|entry| entry["name"] == "orchestrator")
            .unwrap()["scope"],
        "root"
    );
    let worker = agents
        .iter()
        .find(|entry| entry["name"] == "worker")
        .unwrap();
    assert_eq!(worker["scope"], "fallback");
    let canonical_fallback = dunce::canonicalize(&fallback).unwrap();
    assert!(
        std::path::Path::new(worker["source_path"].as_str().unwrap())
            .starts_with(&canonical_fallback)
    );

    let skills_output = mars()
        .args([
            "--json",
            "skills",
            "list",
            "--root",
            project.to_str().unwrap(),
            "--fallback-root",
            fallback.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(skills_output.status.success());
    let skills: serde_json::Value = serde_json::from_slice(&skills_output.stdout).unwrap();
    let skills = skills["skills"].as_array().unwrap();
    assert_eq!(
        skills
            .iter()
            .find(|entry| entry["name"] == "planning")
            .unwrap()["scope"],
        "root"
    );
    let review = skills
        .iter()
        .find(|entry| entry["name"] == "review")
        .unwrap();
    assert_eq!(review["scope"], "fallback");
    assert!(
        std::path::Path::new(review["source_path"].as_str().unwrap())
            .starts_with(&canonical_fallback)
    );
}

#[test]
fn fallback_collisions_stay_shadowed_before_filters_and_show_uses_project_copy() {
    let dir = TempDir::new().unwrap();
    let project_agent = "---\nname: worker\ndescription: Project worker\nmode: primary\n---\n";
    let fallback_agent = "---\nname: worker\ndescription: Fallback worker\nmode: subagent\n---\n";
    let project_skill =
        "---\nname: shared\ndescription: Project skill\nmetadata:\n  type: principle\n---\n";
    let fallback_skill =
        "---\nname: shared\ndescription: Fallback skill\nmetadata:\n  type: guardrail\n---\n";
    let project = setup_synced_project(
        &dir,
        "proj",
        "project-src",
        &[("worker", project_agent)],
        &[("shared", project_skill)],
    );
    let fallback = setup_synced_project(
        &dir,
        "fallback",
        "fallback-src",
        &[("worker", fallback_agent)],
        &[("shared", fallback_skill)],
    );

    for filter in [["--mode", "subagent"], ["--source", "fallback-src"]] {
        let output = offline_mars(dir.path())
            .args([
                "--json",
                "agents",
                "list",
                filter[0],
                filter[1],
                "--root",
                project.to_str().unwrap(),
                "--fallback-root",
                fallback.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["agents"], serde_json::json!([]));
    }

    for filter in [["--type", "guardrail"], ["--source", "fallback-src"]] {
        let output = offline_mars(dir.path())
            .args([
                "--json",
                "skills",
                "list",
                filter[0],
                filter[1],
                "--root",
                project.to_str().unwrap(),
                "--fallback-root",
                fallback.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["skills"], serde_json::json!([]));
    }

    let agent_output = offline_mars(dir.path())
        .args([
            "--json",
            "agents",
            "show",
            "worker",
            "--root",
            project.to_str().unwrap(),
            "--fallback-root",
            fallback.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(agent_output.status.success());
    let agent: serde_json::Value = serde_json::from_slice(&agent_output.stdout).unwrap();
    assert_eq!(agent["description"], "Project worker");
    assert_eq!(agent["scope"], "root");

    let skill_output = offline_mars(dir.path())
        .args([
            "--json",
            "skills",
            "show",
            "shared",
            "--root",
            project.to_str().unwrap(),
            "--fallback-root",
            fallback.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(skill_output.status.success());
    let skill: serde_json::Value = serde_json::from_slice(&skill_output.stdout).unwrap();
    assert_eq!(skill["description"], "Project skill");
    assert_eq!(skill["scope"], "root");
}

#[test]
fn fresh_and_nonexistent_fallback_roots_are_silent_noops_for_cli_lists() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "project-src",
        &[("orchestrator", AGENT_CONTENT)],
        &[("planning", SKILL_CONTENT)],
    );
    let fresh = dir.child("fresh-fallback");
    fresh.create_dir_all().unwrap();
    fresh.child("mars.toml").write_str("").unwrap();
    let nonexistent = dir.child("nonexistent-fallback");

    for fallback in [fresh.path(), nonexistent.path()] {
        for (command, envelope, expected_name) in [
            ("agents", "agents", "orchestrator"),
            ("skills", "skills", "planning"),
        ] {
            let output = offline_mars(dir.path())
                .args([
                    "--json",
                    command,
                    "list",
                    "--root",
                    project.to_str().unwrap(),
                    "--fallback-root",
                    fallback.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(
                output.stderr.is_empty(),
                "unexpected stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(
                json[envelope]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["name"] == expected_name)
            );
        }
    }
}

#[test]
fn corrupt_fallback_lock_warns_and_skips_fallback_content() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "project-src",
        &[("orchestrator", AGENT_CONTENT)],
        &[("planning", SKILL_CONTENT)],
    );
    let fallback = setup_synced_project(
        &dir,
        "fallback",
        "fallback-src",
        &[("worker", FALLBACK_AGENT_CONTENT)],
        &[("review", FALLBACK_SKILL_CONTENT)],
    );
    std::fs::write(fallback.join("mars.lock"), "not valid toml = [").unwrap();

    for (command, envelope, kept_name, skipped_name) in [
        ("agents", "agents", "orchestrator", "worker"),
        ("skills", "skills", "planning", "review"),
    ] {
        let output = offline_mars(dir.path())
            .args([
                "--json",
                command,
                "list",
                "--root",
                project.to_str().unwrap(),
                "--fallback-root",
                fallback.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("warning:"), "{stderr}");
        assert!(stderr.contains("mars.lock"), "{stderr}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let entries = json[envelope].as_array().unwrap();
        assert!(entries.iter().any(|entry| entry["name"] == kept_name));
        assert!(!entries.iter().any(|entry| entry["name"] == skipped_name));
    }
}

// ── mars skills --json ────────────────────────────────────────────────────────

#[test]
fn skills_list_json_model_invocable_is_kebab_not_snake() {
    // Regression guard: list previously emitted `model_invocable` (snake) while
    // show emitted `model-invocable` (kebab). Both must be kebab.
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(&dir, "proj", "src", &[], &[("planning", SKILL_CONTENT)]);

    let output = mars()
        .args(["--json", "skills", "--root", project.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success(), "mars skills should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("mars skills --json must be valid JSON:\n{stdout}"));

    let skills = json["skills"]
        .as_array()
        .unwrap_or_else(|| panic!("expected top-level 'skills' array:\n{stdout}"));
    assert!(!skills.is_empty(), "expected at least one skill:\n{stdout}");

    let entry = skills
        .iter()
        .find(|e| e["name"] == "planning")
        .unwrap_or_else(|| panic!("'planning' skill not found in list:\n{stdout}"));

    // The key contract: kebab-case, not snake_case.
    assert!(
        entry.get("model-invocable").is_some(),
        "skills list MUST emit 'model-invocable' (kebab), not 'model_invocable' (snake):\n{stdout}"
    );
    assert!(
        entry.get("model_invocable").is_none(),
        "skills list MUST NOT emit 'model_invocable' (snake):\n{stdout}"
    );
    assert_eq!(
        entry["model-invocable"], false,
        "'model-invocable' value for planning:\n{stdout}"
    );

    assert_eq!(
        entry["type"], "principle",
        "'type' field in skills list:\n{stdout}"
    );
}

#[test]
fn skills_show_json_model_invocable_is_kebab_not_snake() {
    // Mirror of the list test — both endpoints must agree on kebab-case.
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(&dir, "proj", "src", &[], &[("planning", SKILL_CONTENT)]);

    let output = mars()
        .args([
            "--json",
            "skills",
            "show",
            "planning",
            "--root",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "mars skills show should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("mars skills show --json must be valid JSON:\n{stdout}"));

    // Kebab-case contract — the regression was snake vs kebab disagreement.
    assert!(
        json.get("model-invocable").is_some(),
        "skills show MUST emit 'model-invocable' (kebab):\n{stdout}"
    );
    assert!(
        json.get("model_invocable").is_none(),
        "skills show MUST NOT emit 'model_invocable' (snake):\n{stdout}"
    );
    assert_eq!(
        json["model-invocable"], false,
        "'model-invocable' value:\n{stdout}"
    );

    assert!(
        json.get("user-invocable").is_some(),
        "'user-invocable' key required (kebab):\n{stdout}"
    );
    assert_eq!(json["type"], "principle", "'type' field:\n{stdout}");
    assert!(
        json.get("detail").is_none(),
        "'detail' field should not be emitted:\n{stdout}"
    );
}

#[test]
fn skills_show_not_found_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(&dir, "proj", "src", &[], &[("planning", SKILL_CONTENT)]);

    let status = mars()
        .args([
            "--json",
            "skills",
            "show",
            "nonexistent",
            "--root",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap()
        .status;

    assert!(
        !status.success(),
        "skills show of missing skill must exit non-zero"
    );
}
