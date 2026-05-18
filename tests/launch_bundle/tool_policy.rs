// qa-validated: launch-bundle-test-cleanup

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
        serde_json::json!(["shell(meridian spawn *)"])
    );
    assert_eq!(
        override_bundle["tools"]["disallowed"],
        serde_json::json!(["agent", "file_write"])
    );
    assert_eq!(
        override_bundle["tools"]["mcp"],
        serde_json::json!(["plugin:override"])
    );
}

pub(crate) fn build_launch_bundle_normalizes_tool_head_and_preserves_scoped_payload() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  "bash(git status *)": allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash(git status *)"])
    );
}

pub(crate) fn build_launch_bundle_warns_for_unknown_first_class_tool_and_preserves_mcp() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  plan_mode: deny
  notebook: allow
mcp-tools:
  - plugin:context7:context7
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["tools"]["allowed"], serde_json::json!(["Notebook"]));
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["plan_mode"])
    );
    assert_eq!(
        bundle["tools"]["mcp"],
        serde_json::json!(["plugin:context7:context7"])
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'plan_mode' is not a known claude tool")
    }));
}

pub(crate) fn build_launch_bundle_opencode_tool_normalization_maps_web_aliases_and_warns_unknown() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  read: allow
  Write: allow
  edit: deny
  Agent: deny
  web_search: allow
  web_fetch: allow
  plan_mode: deny
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
        "--harness",
        "opencode",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["bash", "read", "write", "browser", "fetch"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["edit", "agent", "plan_mode"])
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'plan_mode' is not a known opencode tool")
    }));
}

pub(crate) fn build_launch_bundle_cursor_and_pi_unknown_tools_pass_silently() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  web_search: allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    for harness in ["cursor", "pi"] {
        let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
        cmd.args([
            "build",
            "launch-bundle",
            "--agent",
            "reviewer",
            "--harness",
            harness,
        ]);

        let output = cmd.assert().success().get_output().clone();
        let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            bundle["tools"]["allowed"],
            serde_json::json!(["web_search"])
        );

        let warnings = bundle["warnings"]
            .as_array()
            .expect("warnings should be an array");
        assert!(!warnings.iter().any(|warning| {
            warning
                .as_str()
                .unwrap_or_default()
                .contains("tool 'web_search' is not a known")
        }));
    }
}
