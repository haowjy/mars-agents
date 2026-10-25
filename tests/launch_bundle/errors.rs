use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;

#[test]
fn build_launch_bundle_accepts_model_invocable_passthrough_override() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  claude:
    model-invocable: false
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    let output = cmd.assert().success().get_output().clone();
    let bundle: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["execution_policy"]["native_config"]["model-invocable"].as_bool(),
        Some(false)
    );
}

#[test]
fn build_launch_bundle_fails_when_agent_file_missing() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "missing-agent"]);
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("missing-agent"))
        .stderr(predicates::str::contains("read launch bundle agent"));
}
