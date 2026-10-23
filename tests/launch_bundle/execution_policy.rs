use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

#[test]
fn build_launch_bundle_cli_overrides_profile_execution_policy_fields() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: low
approval: confirm
sandbox: read-only
network: deny
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
        "never",
        "--sandbox",
        "danger-full-access",
        "--network",
        "allow",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("never")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("danger-full-access")
    );
    assert_eq!(bundle["execution_policy"]["network"], "allow");
    assert_eq!(bundle["provenance"]["effort_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(bundle["provenance"]["sandbox_source"].as_str(), Some("cli"));
    assert_eq!(bundle["provenance"]["network_source"], "cli");
}

#[test]
fn build_launch_bundle_harness_override_is_passthrough_not_execution_policy() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: modelalias
effort: low
approval: confirm
sandbox: read-only
network: deny
autocompact: 1200
autocompact_pct: 40
harness-overrides:
  codex:
    effort: high
    approval: auto
    sandbox: workspace-write
    network: allow
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
        "never",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("low"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("never"),
        "CLI approval must beat profile and passthrough",
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("read-only")
    );
    assert_eq!(bundle["execution_policy"]["network"], "deny");
    assert_eq!(
        bundle["execution_policy"]["autocompact"].as_u64(),
        Some(1200)
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact_pct"].as_u64(),
        Some(40)
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(
        bundle["provenance"]["sandbox_source"].as_str(),
        Some("profile")
    );
    assert_eq!(bundle["provenance"]["network_source"], "profile");
    assert_eq!(
        bundle["provenance"]["autocompact_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["autocompact_pct_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["execution_policy"]["native_config"]["effort"].as_str(),
        Some("high")
    );
    assert_eq!(
        bundle["execution_policy"]["native_config"]["native-config"]
            ["sandbox_workspace_write.network_access"]
            .as_bool(),
        Some(true)
    );
}

#[test]
fn build_launch_bundle_profile_execution_policy_flows_without_cli_override() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: xhigh
approval: auto
sandbox: workspace-write
network: allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("xhigh"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("auto")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(bundle["execution_policy"]["network"], "allow");
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
    assert_eq!(bundle["provenance"]["network_source"], "profile");
}

#[test]
fn build_launch_bundle_warns_when_network_deny_cannot_be_enforced() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
sandbox: danger-full-access
network: deny
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(
        bundle["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning.as_str().is_some_and(|message| {
                    message.contains("no harness can enforce network isolation")
                })
            })
    );
}

#[test]
fn build_launch_bundle_rejects_network_default_from_overlay() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
network: deny
---
Review code changes."#;
    let extra_toml = r#"[agents.reviewer]
network = "default""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains(
            "network execution policy has invalid value `default`",
        ))
        .stderr(predicates::str::contains("allowed: allow, deny"));
}
