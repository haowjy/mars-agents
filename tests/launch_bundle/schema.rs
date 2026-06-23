use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, configure_assert_cmd, mars, mars_cmd, sample_catalog_json};
use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::MockServer;
use httpmock::prelude::*;
use serde_json::Value;

fn assert_field_absent_or_null(bundle: &Value, field: &str) {
    assert!(
        bundle.get(field).is_none() || bundle[field].is_null(),
        "{field} should be absent or null"
    );
}

pub(crate) fn build_launch_bundle_outputs_schema_and_slot_placeholders() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
tools: [Bash, Write]
disallowed-tools: [Agent]
mcp-tools: [plugin:context7:context7]
---

Review code changes.
"#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to plan.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();

    let bundle: Value = serde_json::from_str(&stdout).expect("launch-bundle should emit JSON");

    assert_eq!(bundle["version"].as_u64(), Some(3));
    assert_eq!(bundle["agent"].as_str(), Some("reviewer"));
    assert_eq!(
        bundle["agent_body"].as_str(),
        Some("\nReview code changes.\n")
    );
    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Agent Profile\n\nReview code changes.\n\n"));
    assert!(!system_instruction.contains("# Agent Profile\n\n\nReview code changes.\n\n"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert!(bundle["routing"]["selection_kind"].is_string());
    assert!(bundle["routing"]["match_evidence"].is_string());
    assert!(bundle["routing"]["harness_model"].is_string());
    assert!(bundle["routing"]["harness_model_source"].is_string());
    assert!(bundle["routing"]["harness_model_confidence"].is_string());
    assert_eq!(
        bundle["routing"]["route_trace"]["version"].as_u64(),
        Some(1)
    );
    assert!(bundle["provenance"]["selection_kind"].is_string());
    assert!(bundle["provenance"]["match_evidence"].is_string());
    assert!(bundle["provenance"]["candidates_tried"].is_string());
    assert!(bundle["execution_policy"]["codex_rules"].is_null());
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["exec_command", "apply_patch"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["spawn_agent"])
    );
    assert_eq!(bundle["tools"]["mcp"], serde_json::json!([]));
    assert!(
        bundle["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap_or_default()
                .contains("cannot be represented for codex"))
    );
    assert!(bundle["provenance"]["harness_stability"].is_null());

    for slot in [
        "completion_contract",
        "context_prompt",
        "user_prompt_file",
        "context_files",
        "prior_session_context",
        "spawn_metadata",
    ] {
        assert_eq!(bundle["scaffold_slots"][slot].as_str(), Some("###SLOT###"));
    }
}

pub(crate) fn build_launch_bundle_supports_ad_hoc_mode_with_model_override() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
tools: [Bash, Write]
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to plan.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--model", "gpt-5.4-mini"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["version"].as_u64(), Some(3));
    assert!(bundle["agent"].is_null());
    assert_field_absent_or_null(&bundle, "agent_body");
    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("gpt-5.4-mini")
    );
    assert!(bundle["routing"]["harness"].is_string());
    assert_eq!(bundle["tools"]["allowed"], serde_json::json!([]));
    assert_eq!(bundle["tools"]["disallowed"], serde_json::json!([]));
    assert_eq!(bundle["tools"]["mcp"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["loaded"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));
    assert_eq!(
        bundle["prompt_surface"]["supplemental_documents"],
        serde_json::json!(Vec::<Value>::new())
    );
}

pub(crate) fn build_launch_bundle_ad_hoc_without_mars_toml() {
    let temp = TempDir::new().unwrap();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    let project = temp.child("plain-project");
    project.create_dir_all().unwrap();

    let mut cmd = mars();
    configure_assert_cmd(&mut cmd, temp.path(), &server.url(API_PATH));
    cmd.current_dir(project.path())
        .env("PATH", replace_path_with(&bin_dir))
        .args([
            "build",
            "launch-bundle",
            "--model",
            "gpt-5.4-mini",
            "--harness",
            "pi",
        ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(bundle["agent"].is_null());
    assert_field_absent_or_null(&bundle, "agent_body");
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("cached-probe")
    );
    assert_eq!(bundle["warnings"], serde_json::json!([]));
}

pub(crate) fn build_launch_bundle_ad_hoc_supports_skills_missing_metadata_and_execution_overrides()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to plan.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--model",
        "gpt-5",
        "--skill",
        "planning,missing_skill",
        "--effort",
        "high",
        "--approval",
        "auto",
        "--sandbox",
        "workspace-write",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(bundle["agent"].is_null());
    assert_eq!(
        bundle["skills"]["loaded"][0]["name"].as_str(),
        Some("planning")
    );
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(
        bundle["skills"]["missing"],
        serde_json::json!(["missing_skill"])
    );

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("planning"));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: planning"));
    assert!(!system_instruction.contains("Review code changes."));

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("auto")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gpt-5"));
    assert!(bundle["routing"]["harness"].is_string());
    assert!(bundle["routing"]["selection_kind"].is_string());
    assert!(bundle["routing"]["match_evidence"].is_string());
    assert!(bundle["routing"]["harness_model"].is_string());
}

pub(crate) fn build_launch_bundle_rejects_prompt_file_flag() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--prompt-file",
        "task.md",
    ]);

    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("--prompt-file"));
}

pub(crate) fn build_launch_bundle_uses_installed_harness_default_when_no_model_available() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some(""));
    assert_eq!(bundle["routing"]["model"].as_str(), Some(""));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["routing"]["harness_model"].as_str(), Some(""));
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("passthrough")
    );
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("unset"));
}

pub(crate) fn build_launch_bundle_ad_hoc_without_model_uses_installed_harness_default() {
    let temp = TempDir::new().unwrap();
    let server = MockServer::start();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let project = temp.child("plain-project");
    project.create_dir_all().unwrap();

    let mut cmd = mars();
    configure_assert_cmd(&mut cmd, temp.path(), &server.url(API_PATH));
    cmd.current_dir(project.path())
        .env("PATH", replace_path_with(&bin_dir));
    cmd.args(["build", "launch-bundle"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(bundle["agent"].is_null());
    assert_field_absent_or_null(&bundle, "agent_body");
    assert_eq!(bundle["routing"]["model_token"].as_str(), Some(""));
    assert_eq!(bundle["routing"]["model"].as_str(), Some(""));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(bundle["routing"]["harness_model"].as_str(), Some(""));
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("unset"));
}

pub(crate) fn build_launch_bundle_resolves_model_alias_from_consumer_config() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: bundlealias
---
Review code changes."#;

    let extra_toml = r#"[models.bundlealias]
model = "openai/gpt-5"
harness = "codex"
default_effort = "high"
autocompact = 24000
autocompact_pct = 70"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("bundlealias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai/gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["autocompact"].as_u64(),
        Some(24000)
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact_pct"].as_u64(),
        Some(70)
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
}
