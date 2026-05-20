use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_cli_overrides_profile_execution_policy_fields() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: low
approval: confirm
sandbox: read-only
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
        "--effort",
        "high",
        "--approval",
        "yolo",
        "--sandbox",
        "danger-full-access",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("yolo")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("danger-full-access")
    );
    assert_eq!(bundle["provenance"]["effort_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(bundle["provenance"]["sandbox_source"].as_str(), Some("cli"));
}

pub(crate) fn build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias()
 {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: modelalias
effort: low
approval: confirm
sandbox: read-only
autocompact: 1200
autocompact_pct: 40
harness-overrides:
  codex:
    effort: high
    approval: auto
    sandbox: workspace-write
    autocompact: 2400
    autocompact_pct: 70
    native-config:
      sandbox_workspace_write.network_access: true
---
Review code changes."#;

    let extra_toml = r#"[models.modelalias]
model = "openai/gpt-5"
harness = "codex"
default_effort = "medium"
autocompact = 9000
autocompact_pct = 55"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--approval",
        "yolo",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("yolo"),
        "CLI approval must beat harness override",
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact"].as_u64(),
        Some(2400)
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact_pct"].as_u64(),
        Some(70)
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(
        bundle["provenance"]["sandbox_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["autocompact_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["autocompact_pct_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile-harness-override")
    );
}

pub(crate) fn build_launch_bundle_profile_execution_policy_flows_without_cli_override() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: xhigh
approval: auto
sandbox: workspace-write
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("xhigh"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("auto")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["sandbox_source"].as_str(),
        Some("profile")
    );
}
