// qa-validated: launch-bundle-test-cleanup

use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_passthrough()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  Agent: deny
  Edit: deny
  mcp(plugin:root): allow
disallowed-tools: [Write]
harness-overrides:
  codex:
    tools:
      "Bash(meridian spawn *)": allow
      Agent: deny
    disallowed-tools: [Edit]
    mcp-tools: [plugin:override]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut root_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    root_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    root_cmd.env("PATH", replace_path_with(&bin_dir));
    let root_output = root_cmd.assert().success().get_output().clone();
    let root_bundle: Value = serde_json::from_slice(&root_output.stdout).unwrap();
    assert_eq!(root_bundle["tools"]["allowed"], serde_json::json!(["Bash"]));
    assert_eq!(
        root_bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit", "Write"])
    );
    assert_eq!(
        root_bundle["tools"]["mcp"],
        serde_json::json!(["mcp__plugin:root__*"])
    );

    let mut override_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    override_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    override_cmd.env("PATH", replace_path_with(&bin_dir));
    let override_output = override_cmd.assert().success().get_output().clone();
    let override_bundle: Value = serde_json::from_slice(&override_output.stdout).unwrap();
    assert_eq!(
        override_bundle["tools"]["allowed"],
        serde_json::json!(["exec_command"])
    );
    assert_eq!(
        override_bundle["tools"]["disallowed"],
        serde_json::json!(["spawn_agent", "apply_patch"])
    );
    assert_eq!(override_bundle["tools"]["mcp"], serde_json::json!([]));
    assert!(
        override_bundle["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning
                .as_str()
                .unwrap_or_default()
                .contains("MCP ref `mcp(plugin:root/*)` cannot be represented for codex"))
    );
    assert_eq!(
        override_bundle["execution_policy"]["native_config"]["mcp-tools"],
        serde_json::json!(["plugin:override"])
    );
}

pub(crate) fn build_launch_bundle_normalizes_tool_head_and_preserves_scoped_payload() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  "Bash(git status *)": allow
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
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash(git status *)"])
    );
}

pub(crate) fn build_launch_bundle_warns_for_unknown_first_class_tool_and_preserves_mcp() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  PlanMode: deny
  CustomDeny: deny
  CustomTool: allow
  Notebook: allow
  mcp(plugin:context7:context7): allow
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
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["CustomTool", "Notebook"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["PlanMode", "CustomDeny"])
    );
    assert_eq!(
        bundle["tools"]["mcp"],
        serde_json::json!(["mcp__plugin:context7:context7__*"])
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'custom_tool' is not a known claude tool")
    }));
    assert!(!warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'PlanMode' is not a known claude tool")
    }));
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("disallowed tool 'custom_deny' is not a known claude tool")
    }));
}

pub(crate) fn build_launch_bundle_opencode_tool_normalization_maps_web_aliases_and_warns_unknown() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  Read: allow
  Write: allow
  CustomTool: allow
  Edit: deny
  Agent: deny
  WebSearch: allow
  WebFetch: allow
  PlanMode: deny
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
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["bash", "view", "write", "custom_tool", "browser", "fetch"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["edit", "agent", "planmode"])
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'custom_tool' is not a known opencode tool")
    }));
    assert!(!warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("disallowed tool 'plan_mode' is not a known opencode tool")
    }));
}

pub(crate) fn build_launch_bundle_skill_deny_projects_without_unknown_warning() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  'skill(init)': deny
  workflow: deny
  web: allow
  definitely-not-a-tool: deny
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut claude_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    claude_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    claude_cmd.env("PATH", replace_path_with(&bin_dir));
    let claude_output = claude_cmd.assert().success().get_output().clone();
    let claude_bundle: Value = serde_json::from_slice(&claude_output.stdout).unwrap();
    assert_eq!(
        claude_bundle["tools"]["allowed"],
        serde_json::json!(["Bash", "WebSearch"])
    );
    assert_eq!(
        claude_bundle["tools"]["disallowed"],
        serde_json::json!(["Skill(init)", "Workflow", "Definitely-not-a-tool"])
    );
    let claude_warnings = claude_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(!claude_warnings.iter().any(|warning| {
        let text = warning.as_str().unwrap_or_default();
        text.contains("skill(init)") || text.contains("workflow") || text.contains("'web'")
    }));
    assert!(claude_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("disallowed tool 'definitely-not-a-tool' is not a known claude tool")
    }));

    let mut codex_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    codex_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    codex_cmd.env("PATH", replace_path_with(&bin_dir));
    let codex_output = codex_cmd.assert().success().get_output().clone();
    let codex_bundle: Value = serde_json::from_slice(&codex_output.stdout).unwrap();
    assert_eq!(
        codex_bundle["tools"]["allowed"],
        serde_json::json!(["exec_command", "web_search"])
    );
    assert_eq!(
        codex_bundle["tools"]["disallowed"],
        serde_json::json!(["skill(init)", "workflow", "definitely-not-a-tool"])
    );
    let codex_warnings = codex_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(!codex_warnings.iter().any(|warning| {
        let text = warning.as_str().unwrap_or_default();
        text.contains("skill(init)") || text.contains("workflow") || text.contains("'web'")
    }));
    assert!(codex_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("disallowed tool 'definitely-not-a-tool' is not a known codex tool")
    }));
}

pub(crate) fn build_launch_bundle_cursor_and_pi_unknown_tools_warn_and_pass_through() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor", "pi"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  CustomTool: allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cursor_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cursor_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "cursor",
    ]);
    cursor_cmd.env("PATH", replace_path_with(&bin_dir));
    let cursor_output = cursor_cmd.assert().success().get_output().clone();
    let cursor_bundle: Value = serde_json::from_slice(&cursor_output.stdout).unwrap();
    assert_eq!(
        cursor_bundle["tools"]["allowed"],
        serde_json::json!(["CustomTool"])
    );
    let cursor_warnings = cursor_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(cursor_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'custom_tool' is not a known")
    }));

    let mut pi_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    pi_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "pi",
    ]);
    pi_cmd.env("PATH", replace_path_with(&bin_dir));
    let pi_output = pi_cmd.assert().success().get_output().clone();
    let pi_bundle: Value = serde_json::from_slice(&pi_output.stdout).unwrap();
    assert_eq!(
        pi_bundle["tools"]["allowed"],
        serde_json::json!(["custom_tool"])
    );
    let pi_warnings = pi_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(pi_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("tool 'custom_tool' is not a known")
    }));
}

pub(crate) fn build_launch_bundle_projects_mcp_refs_per_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools: [mcp(github/create_issue)]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut claude_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    claude_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    claude_cmd.env("PATH", replace_path_with(&bin_dir));
    let claude_output = claude_cmd.assert().success().get_output().clone();
    let claude_bundle: Value = serde_json::from_slice(&claude_output.stdout).unwrap();
    assert_eq!(
        claude_bundle["tools"]["mcp"],
        serde_json::json!(["mcp__github__create_issue"])
    );
    let claude_warnings = claude_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(!claude_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("MCP ref `mcp(github/create_issue)` cannot be represented")
    }));

    let mut codex_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    codex_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    codex_cmd.env("PATH", replace_path_with(&bin_dir));
    let codex_output = codex_cmd.assert().success().get_output().clone();
    let codex_bundle: Value = serde_json::from_slice(&codex_output.stdout).unwrap();
    assert_eq!(codex_bundle["tools"]["mcp"], serde_json::json!([]));
    let codex_warnings = codex_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(codex_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("MCP ref `mcp(github/create_issue)` cannot be represented")
    }));
}

pub(crate) fn build_launch_bundle_projects_disallowed_mcp_refs_per_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
disallowed-tools: [mcp(github/delete_repo)]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cursor_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cursor_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "cursor",
    ]);
    cursor_cmd.env("PATH", replace_path_with(&bin_dir));
    let cursor_output = cursor_cmd.assert().success().get_output().clone();
    let cursor_bundle: Value = serde_json::from_slice(&cursor_output.stdout).unwrap();
    assert_eq!(cursor_bundle["tools"]["mcp"], serde_json::json!([]));
    assert_eq!(
        cursor_bundle["tools"]["disallowed"],
        serde_json::json!(["Mcp(github:delete_repo)"])
    );
    let cursor_warnings = cursor_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(!cursor_warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("disallowed MCP ref")
    }));

    let mut codex_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    codex_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    codex_cmd.env("PATH", replace_path_with(&bin_dir));
    let codex_output = codex_cmd.assert().success().get_output().clone();
    let codex_bundle: Value = serde_json::from_slice(&codex_output.stdout).unwrap();
    assert_eq!(codex_bundle["tools"]["mcp"], serde_json::json!([]));
    assert_eq!(codex_bundle["tools"]["disallowed"], serde_json::json!([]));
    let codex_warnings = codex_bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(codex_warnings.iter().any(|warning| {
        warning.as_str().unwrap_or_default().contains(
            "disallowed MCP ref `mcp(github/delete_repo)` cannot be represented for codex",
        )
    }));
}
