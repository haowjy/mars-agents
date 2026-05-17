mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use serde_json::Value;

use common::{API_PATH, create_source, mars_cmd, sample_catalog_json};

fn setup_bundle_project(
    temp: &TempDir,
    source_name: &str,
    agent_content: &str,
    skills: &[(&str, &str)],
    extra_project_toml: &str,
) -> (MockServer, std::path::PathBuf) {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let source = create_source(temp, source_name, &[("reviewer", agent_content)], skills);
    let project = temp.child("project");
    project.create_dir_all().unwrap();

    let mut toml = format!(
        "[dependencies]\n{source_name} = {{ path = \"{}\" }}\n",
        source.display().to_string().replace('\\', "/")
    );
    if !extra_project_toml.trim().is_empty() {
        toml.push('\n');
        toml.push_str(extra_project_toml);
        toml.push('\n');
    }
    project.child("mars.toml").write_str(&toml).unwrap();

    let mut sync_cmd = mars_cmd(project.path(), temp.path(), &server.url(API_PATH));
    sync_cmd.arg("sync");
    sync_cmd.assert().success();

    (server, project.to_path_buf())
}

#[test]
fn build_launch_bundle_outputs_schema_and_slot_placeholders() {
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
    assert!(stdout.contains("\n  \"version\": 1"));

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

#[test]
fn build_launch_bundle_includes_skill_documents_and_system_instruction() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to reason about steps.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("planning"));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: planning"));
    assert!(system_instruction.contains("Use this skill to reason about steps."));
    assert!(system_instruction.contains("# Report Contract"));
    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["planning"])
    );
    assert_eq!(bundle["skills_metadata"]["missing"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_rejects_prompt_file_flag() {
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

#[test]
fn build_launch_bundle_resolves_model_alias_from_consumer_config() {
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
