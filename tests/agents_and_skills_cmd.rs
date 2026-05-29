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
type: principle
model-invocable: false
---
# Planning
";

// ── mars agents --json ────────────────────────────────────────────────────────

#[test]
fn agents_list_json_envelope_and_fields() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[("orchestrator", AGENT_CONTENT)],
        &[],
    );

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
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[("orchestrator", AGENT_CONTENT)],
        &[],
    );

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
    assert!(json["skills"].is_array(), "'skills' array required:\n{stdout}");
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

    // Kebab-case keys in the show envelope
    assert!(
        json.get("disallowed-tools").is_some(),
        "'disallowed-tools' key required (kebab):\n{stdout}"
    );
    assert!(
        json.get("tools-denied").is_some(),
        "'tools-denied' key required (kebab):\n{stdout}"
    );
    assert!(
        json.get("mcp-tools").is_some(),
        "'mcp-tools' key required (kebab):\n{stdout}"
    );
}

#[test]
fn agents_show_not_found_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[("orchestrator", AGENT_CONTENT)],
        &[],
    );

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

// ── mars skills --json ────────────────────────────────────────────────────────

#[test]
fn skills_list_json_model_invocable_is_kebab_not_snake() {
    // Regression guard: list previously emitted `model_invocable` (snake) while
    // show emitted `model-invocable` (kebab). Both must be kebab.
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[],
        &[("planning", SKILL_CONTENT)],
    );

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
        entry["model-invocable"],
        false,
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
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[],
        &[("planning", SKILL_CONTENT)],
    );

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
        json["model-invocable"],
        false,
        "'model-invocable' value:\n{stdout}"
    );

    assert!(
        json.get("user-invocable").is_some(),
        "'user-invocable' key required (kebab):\n{stdout}"
    );
    assert!(
        json.get("allowed-tools").is_some(),
        "'allowed-tools' key required (kebab):\n{stdout}"
    );
    assert_eq!(json["type"], "principle", "'type' field:\n{stdout}");
    assert!(json["detail"].is_string(), "'detail' field required:\n{stdout}");
}

#[test]
fn skills_show_not_found_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let project = setup_synced_project(
        &dir,
        "proj",
        "src",
        &[],
        &[("planning", SKILL_CONTENT)],
    );

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
