use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_omits_launch_actions_without_context() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: cursor
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["version"].as_u64(), Some(4));
    assert!(bundle.get("launch_actions").is_none());
}

pub(crate) fn build_launch_bundle_projects_cursor_launch_actions() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: cursor
approval: auto
sandbox: read-only
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let context = serde_json::json!({
        "cwd": "/work/project",
        "temp_dir": "/tmp/mars-spawn",
        "streaming": null,
        "session_id": null,
        "fork": false,
        "workspace_roots": ["/ignored-extra-root"],
        "interactive": false,
        "extra_args": ["--foo"],
        "opencode_config_content": null,
        "pi_extension_entrypoints": [],
        "prompt": "Review this change"
    });

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--context",
        &context.to_string(),
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["launch_actions"]["argv"],
        serde_json::json!([
            "cursor",
            "agent",
            "--print",
            "--output-format",
            "stream-json",
            "--trust",
            "--model",
            "claude-opus-4-6",
            "--force",
            "--sandbox",
            "enabled",
            "--workspace",
            "/work/project",
            "--foo",
            "Review this change"
        ])
    );
    assert_eq!(bundle["launch_actions"]["env"], serde_json::json!({}));
    assert_eq!(bundle["launch_actions"]["files"], serde_json::json!([]));
    assert!(bundle["launch_actions"]["protocol_payload"].is_null());
}
