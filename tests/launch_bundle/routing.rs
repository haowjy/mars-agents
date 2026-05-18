// qa-validated: harness-order-settings-audit

use super::common::setup_bundle_project;
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn build_launch_bundle_cli_model_alias_harness_beats_profile_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[models.bundlealias]
model = "openai/gpt-5"
harness = "codex""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "bundlealias",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("bundlealias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai/gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
}

pub(crate) fn build_launch_bundle_cli_model_override_uses_provider_harness_before_profile_harness()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
model = "gpt-5""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "openai_alias",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("openai_alias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex")
    );
}

pub(crate) fn build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: openai_alias
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
model = "gpt-5""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(bundle["routing"]["harness_model"].as_str(), Some("gpt-5"));
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("provider-match")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("likely")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex")
    );
}

pub(crate) fn build_launch_bundle_uses_alias_provider_when_auto_resolve_misses_model_cache() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: openai_alias
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
provider = "openai"
match = ["definitely-not-a-cached-openai-model-*"]"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai_alias"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("openai_alias")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("unknown")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("did not resolve from cached catalog")
    }));
}

pub(crate) fn build_launch_bundle_uses_settings_default_harness_before_hardcoded_fallback() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &[]);
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_harness = "pi""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model"].as_str(),
        Some("unknown-model-token")
    );
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode,cursor")
    );
    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("unknown-model-token")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("unknown")
    );
}

pub(crate) fn build_launch_bundle_cli_direct_model_id_prefers_provider_harness_over_profile() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
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
        "--model",
        "gpt-5",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
}

pub(crate) fn build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &[]);
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_harness = "invalid-harness""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode,cursor")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("settings.default_harness")
    }));
}

pub(crate) fn build_launch_bundle_provider_fallback_skips_non_launch_bundle_harnesses() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["gemini"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
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
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("claude,pi,opencode,cursor")
    );
    assert_ne!(bundle["routing"]["harness"].as_str(), Some("gemini"));
}

pub(crate) fn build_launch_bundle_uses_settings_harness_order_before_default_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["pi", "opencode", "codex"]
default_harness = "claude""#;
    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config-order")
    );
    assert_eq!(
        bundle["provenance"]["harness_order_position"].as_str(),
        Some("1")
    );
}

pub(crate) fn build_launch_bundle_cli_harness_override_beats_settings_harness_order() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["pi", "opencode"]"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["provenance"]["harness_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("explicit")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("explicit")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(!warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("settings.harness_order is set but none")
    }));
}

pub(crate) fn build_launch_bundle_profile_harness_beats_settings_harness_order() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["codex", "opencode"]"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("profile")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());
}

pub(crate) fn build_launch_bundle_alias_harness_beats_settings_harness_order() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: bundlealias
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["pi", "opencode"]

[models.bundlealias]
model = "gpt-5"
harness = "codex""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("bundlealias")
    );
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());
}

pub(crate) fn build_launch_bundle_cli_model_override_uses_settings_harness_order_before_profile_harness()
 {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["pi", "opencode"]"#;
    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "gpt-5",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config-order")
    );
    assert_eq!(
        bundle["provenance"]["harness_order_position"].as_str(),
        Some("1")
    );
}

pub(crate) fn build_launch_bundle_all_invalid_harness_order_warns_and_falls_through_to_default_harness()
 {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["bad-one", "bad-two"]
default_harness = "pi""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config")
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        let message = warning.as_str().unwrap_or_default();
        message.contains("settings.harness_order contains unrecognized harness")
            && message.contains("bad-one")
    }));
    assert!(warnings.iter().any(|warning| {
        let message = warning.as_str().unwrap_or_default();
        message.contains("settings.harness_order contains unrecognized harness")
            && message.contains("bad-two")
    }));
    assert!(
        !warnings
            .iter()
            .any(|warning| { warning.as_str().unwrap_or_default().contains("none of [") })
    );
}

pub(crate) fn build_launch_bundle_harness_order_none_installed_uses_default_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["pi", "opencode"]
default_harness = "claude""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("settings.harness_order is set but none of [pi, opencode] are installed")
    }));
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("falling through to settings.default_harness")
    }));
}

pub(crate) fn build_launch_bundle_resolves_harness_model_from_cached_opencode_probe() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;
    let extra_toml = r#"[models.gpt55]
model = "gpt-5.5""#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.5", "openai/gpt-5"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "gpt55",
        "--harness",
        "opencode",
    ]);
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("openai/gpt-5.5")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("cached-probe")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("confirmed")
    );
}

pub(crate) fn build_launch_bundle_openai_falls_back_to_pi_when_codex_missing() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi")
    );
}

pub(crate) fn build_launch_bundle_openai_falls_back_to_pi_when_codex_auth_fails() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses_with_auth_failures(&temp, &["codex", "pi"], &["codex"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi")
    );
}

pub(crate) fn build_launch_bundle_google_model_prefers_pi_and_never_gemini_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "gemini"]);
    let agent_content = r#"---
name: reviewer
model: gemini-2.5-pro
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_ne!(bundle["routing"]["harness"].as_str(), Some("gemini"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi")
    );
}

pub(crate) fn build_launch_bundle_openai_falls_back_to_opencode_with_cached_capability_evidence() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.4-mini"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("likely")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("likely")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi,opencode")
    );
}

pub(crate) fn build_launch_bundle_prefers_pi_over_opencode_even_with_positive_opencode_cache() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.4-mini"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi")
    );
}

pub(crate) fn build_launch_bundle_prefers_opencode_before_cursor_when_both_installed() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode", "cursor"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.4-mini"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi,opencode")
    );
}

pub(crate) fn build_launch_bundle_falls_back_to_cursor_when_opencode_cache_is_negative() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode", "cursor"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": false },
            "model_slugs": [],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi,opencode,cursor")
    );
}

pub(crate) fn build_launch_bundle_selects_opencode_when_opencode_cache_is_stale() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode", "cursor"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs().saturating_sub(120),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.4-mini"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("likely")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("codex,pi,opencode")
    );
}

pub(crate) fn build_launch_bundle_unknown_model_prefers_opencode_over_cursor_when_installed() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: third-party-model-123
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode")
    );
}

pub(crate) fn build_launch_bundle_settings_harness_order_runs_gate_checks_before_selection() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode", "pi"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["opencode", "pi"]"#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": false },
            "model_slugs": [],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }),
    );

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_CACHE_DIR", &cache_root);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config-order")
    );
    assert_eq!(
        bundle["provenance"]["harness_order_position"].as_str(),
        Some("1")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("opencode,pi")
    );
}

pub(crate) fn build_launch_bundle_settings_default_harness_accepts_case_insensitive_name() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &[]);
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_harness = "Pi""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config")
    );
}

pub(crate) fn build_launch_bundle_synthesizes_opencode_model_when_cache_missing() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: gpt5
---
Review code changes."#;
    let extra_toml = r#"[models.gpt5]
model = "gpt-5""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

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
        bundle["routing"]["harness_model"].as_str(),
        Some("openai/gpt-5")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("synthesized")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("likely")
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("using synthesized path")
    }));
}

pub(crate) fn build_launch_bundle_unknown_harness_model_path_warns_and_passes_through() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
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
        bundle["routing"]["harness_model"].as_str(),
        Some("unknown-model-token")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("unknown")
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("using passthrough path")
    }));
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("is unconfirmed")
    }));
}

fn install_fake_harnesses(temp: &TempDir, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp.path().join("harness-bin");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        #[cfg(windows)]
        {
            fs::write(
                bin_dir.join(format!("{harness}.bat")),
                "@echo off\r\nexit /b 0\r\n",
            )
            .unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin_dir.join(harness);
            fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    bin_dir
}

fn install_fake_harnesses_with_auth_failures(
    temp: &TempDir,
    harnesses: &[&str],
    auth_failures: &[&str],
) -> PathBuf {
    let bin_dir = temp.path().join("harness-bin-auth");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        let fail_auth = auth_failures.contains(harness);
        #[cfg(windows)]
        {
            let script = if fail_auth && *harness == "codex" {
                "@echo off\r\nif \"%~1 %~2\"==\"login status\" exit /b 1\r\nexit /b 0\r\n"
            } else if fail_auth && *harness == "claude" {
                "@echo off\r\nif \"%~1 %~2\"==\"auth status\" exit /b 1\r\nexit /b 0\r\n"
            } else {
                "@echo off\r\nexit /b 0\r\n"
            };
            fs::write(bin_dir.join(format!("{harness}.bat")), script).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = if fail_auth && *harness == "codex" {
                "#!/bin/sh\nif [ \"$1\" = \"login\" ] && [ \"$2\" = \"status\" ]; then\n  exit 1\nfi\nexit 0\n"
            } else if fail_auth && *harness == "claude" {
                "#!/bin/sh\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then\n  exit 1\nfi\nexit 0\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            };
            let path = bin_dir.join(harness);
            fs::write(&path, script).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
    }

    bin_dir
}

fn replace_path_with(bin_dir: &Path) -> String {
    bin_dir.to_string_lossy().into_owned()
}

fn write_opencode_probe_cache(cache_root: &Path, fetched_at: u64, result: Value) {
    let cache_path = cache_root.join("availability").join("opencode-probe.json");
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let payload = json!({
        "schema_version": 1,
        "fetched_at": fetched_at,
        "last_attempt_at": fetched_at,
        "last_error": null,
        "result": result
    });
    fs::write(cache_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
