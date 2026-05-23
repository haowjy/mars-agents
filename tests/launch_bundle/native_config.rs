use super::common::{
    assert_prompt_surface_excludes, install_fake_harnesses, replace_path_with, setup_bundle_project,
};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
harness-overrides:
  codex:
    native-config:
      sandbox_workspace_write.network_access: true
      approval: "still native"
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
        "codex",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["execution_policy"]["native_config"],
        serde_json::json!({
            "sandbox_workspace_write.network_access": true,
            "approval": "still native"
        })
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile-harness-override")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("collides with a portable field name")
    }));

    assert_prompt_surface_excludes(
        &bundle,
        &["sandbox_workspace_write.network_access", "still native"],
    );
}

pub(crate) fn build_launch_bundle_invalid_native_config_shape_fails_with_diagnostic() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  codex:
    native-config: [1, 2]
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
        "codex",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("native-config"));
}
