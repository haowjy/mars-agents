use super::common::setup_bundle_project;
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement()
{
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  bash: allow
  agent: deny
  edit: deny
disallowed-tools: [write]
mcp-tools: [plugin:root]
harness-overrides:
  codex:
    tools:
      "bash(meridian spawn *)": allow
      agent: deny
    disallowed-tools: [edit]
    mcp-tools: [plugin:override]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut root_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    root_cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    let root_output = root_cmd.assert().success().get_output().clone();
    let root_bundle: Value = serde_json::from_slice(&root_output.stdout).unwrap();
    assert_eq!(root_bundle["tools"]["allowed"], serde_json::json!(["Bash"]));
    assert_eq!(
        root_bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit", "Write"])
    );
    assert_eq!(
        root_bundle["tools"]["mcp"],
        serde_json::json!(["plugin:root"])
    );

    let mut override_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    override_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);
    let override_output = override_cmd.assert().success().get_output().clone();
    let override_bundle: Value = serde_json::from_slice(&override_output.stdout).unwrap();
    assert_eq!(
        override_bundle["tools"]["allowed"],
        serde_json::json!(["Bash(meridian spawn *)"])
    );
    assert_eq!(
        override_bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit"])
    );
    assert_eq!(
        override_bundle["tools"]["mcp"],
        serde_json::json!(["plugin:override"])
    );
}
