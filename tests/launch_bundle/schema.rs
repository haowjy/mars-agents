use super::common::setup_bundle_project;
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_outputs_schema_and_slot_placeholders() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
tools: [Bash, Write]
disallowed-tools: [Agent]
mcp-tools: [plugin:context7:context7]
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
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();

    let bundle: Value = serde_json::from_str(&stdout).expect("launch-bundle should emit JSON");

    assert_eq!(bundle["version"].as_u64(), Some(1));
    assert_eq!(bundle["agent"].as_str(), Some("reviewer"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash", "Write"])
    );
    assert_eq!(bundle["tools"]["disallowed"], serde_json::json!(["Agent"]));
    assert_eq!(
        bundle["tools"]["mcp"],
        serde_json::json!(["plugin:context7:context7"])
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

pub(crate) fn build_launch_bundle_fails_when_no_model_available() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("requires a model"));
}

pub(crate) fn build_launch_bundle_resolves_model_alias_from_consumer_config() {
    let temp = TempDir::new().unwrap();
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
