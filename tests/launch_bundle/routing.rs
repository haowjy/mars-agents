// qa-validated: harness-order-settings-audit
// qa-validated: capability-cache-resolver-routing-gaps

use super::common::setup_bundle_project;
use crate::test_common::{API_PATH, fresh_fetched_at, mars_cmd, write_cache};
use assert_fs::TempDir;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn build_launch_bundle_cli_model_alias_harness_beats_profile_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["claude", "codex"]);
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
    cmd.env("PATH", replace_path_with(&bin_dir));

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
    let bin_dir = install_fake_harnesses(&temp, &["claude", "codex"]);
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
    let bin_dir = install_fake_harnesses(&temp, &["claude", "codex"]);
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
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
}

pub(crate) fn build_launch_bundle_uses_settings_default_model_when_profile_and_cli_missing() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_model = "gpt-5.4-mini""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("gpt-5.4-mini")
    );
    assert_eq!(
        bundle["provenance"]["model_source"].as_str(),
        Some("project")
    );
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
}

pub(crate) fn build_launch_bundle_cli_model_override_beats_settings_default_model() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_model = "gpt-5.4-mini""#;

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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("cli"));
}

pub(crate) fn build_launch_bundle_profile_model_beats_settings_default_model() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_model = "gpt-5.4-mini""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("claude-opus-4-6")
    );
    assert_eq!(
        bundle["provenance"]["model_source"].as_str(),
        Some("profile")
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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
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
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode")
    );
    assert!(bundle["routing"]["route_trace"].is_object());
    assert_eq!(
        bundle["routing"]["route_trace"]["harness"].as_str(),
        Some("opencode")
    );
    let assessments = bundle["routing"]["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    let opencode_assessment = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("opencode"))
        .expect("opencode assessment should exist");
    assert_eq!(
        opencode_assessment["chosen_slug"].as_str(),
        Some("openai/gpt-5")
    );
}

pub(crate) fn build_launch_bundle_provider_order_prefers_configured_provider_over_first_seen_slug()
{
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: gptmini
---
Review code changes."#;

    let extra_toml = r#"[settings]
provider_order = ["openai"]

[models.gptmini]
model = "gpt-5.4-mini""#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "model_slugs": [
                "openrouter/gpt-5.4-mini",
                "openai/gpt-5.4-mini"
            ],
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
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("openai/gpt-5.4-mini")
    );

    let assessments = bundle["routing"]["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    let opencode_assessment = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("opencode"))
        .expect("opencode assessment should exist");
    let candidate_slugs = opencode_assessment["candidate_slugs"]
        .as_array()
        .expect("candidate_slugs should be array");
    assert!(
        candidate_slugs
            .iter()
            .any(|slug| slug.as_str() == Some("openrouter/gpt-5.4-mini"))
    );
    assert!(
        candidate_slugs
            .iter()
            .any(|slug| slug.as_str() == Some("openai/gpt-5.4-mini"))
    );
    assert_eq!(
        opencode_assessment["chosen_slug"].as_str(),
        Some("openai/gpt-5.4-mini")
    );
}

pub(crate) fn build_launch_bundle_nested_slug_model_id_does_not_flatten_into_bare_match() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: gptmini
---
Review code changes."#;

    let extra_toml = r#"[models.gptmini]
model = "gpt-5.4-mini""#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "model_slugs": [
                "openrouter/openai/gpt-5.4-mini",
                "openai/gpt-5.4-mini"
            ],
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
        bundle["routing"]["harness_model"].as_str(),
        Some("openai/gpt-5.4-mini")
    );

    let assessments = bundle["routing"]["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    let opencode_assessment = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("opencode"))
        .expect("opencode assessment should exist");
    let candidate_slugs = opencode_assessment["candidate_slugs"]
        .as_array()
        .expect("candidate_slugs should be array");
    assert!(
        !candidate_slugs
            .iter()
            .any(|slug| slug.as_str() == Some("openrouter/openai/gpt-5.4-mini")),
        "nested slug model id should not match bare gpt-5.4-mini"
    );
    assert_eq!(
        opencode_assessment["chosen_slug"].as_str(),
        Some("openai/gpt-5.4-mini")
    );
}

pub(crate) fn build_launch_bundle_cli_harness_override_beats_settings_harness_order() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode", "codex"]);
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
        Some("forced")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("forced")
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
    let bin_dir = install_fake_harnesses(&temp, &["claude", "codex", "opencode"]);
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
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("forced")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("forced")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());
}

pub(crate) fn build_launch_bundle_unavailable_profile_harness_pivots_to_installed_candidate() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt55
harness: claude
model-policies:
  - match:
      alias: gpt55
    override:
      harness: opencode
---
Review code changes."#;

    let extra_toml = r#"[settings]
harness_order = ["opencode", "codex"]

[models.gpt55]
model = "gpt-5.5"
provider = "openai""#;

    let cache_root = temp.path().join("mars-cache");
    write_opencode_probe_cache(
        &cache_root,
        now_unix_secs(),
        json!({
            "providers": { "openai": true },
            "model_slugs": ["openai/gpt-5.5"],
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
        Some("0")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("constrained")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("constrained")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("opencode")
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning.as_str().unwrap_or_default()
            == "profile harness 'claude' not installed; pivoting via model-policies"
    }));
}

pub(crate) fn build_launch_bundle_unavailable_profile_harness_errors_without_installed_fallback() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    cmd.env("MARS_OFFLINE", "1");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
}

pub(crate) fn build_launch_bundle_unavailable_cli_harness_errors_without_pivoting() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "opencode"]);
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
        "--harness",
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().failure().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert!(stderr.contains("cli harness `claude` is not installed"));
    assert!(stderr.contains("installed harnesses: codex, opencode"));
    assert!(
        !stderr.contains("pivoting via model-policies"),
        "explicit CLI harness must not auto-pivot: {stderr}"
    );
}

pub(crate) fn build_launch_bundle_alias_harness_beats_settings_harness_order() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode", "codex"]);
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
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("forced")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("forced")
    );
    assert!(bundle["provenance"]["harness_order_position"].is_null());
}

pub(crate) fn build_launch_bundle_cli_model_override_uses_settings_harness_order_before_profile_harness()
 {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["claude", "opencode"]);
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
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode")
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
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
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
    cmd.env("PATH", replace_path_with(&bin_dir));

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
        Some("pi")
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
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi")
    );
}

pub(crate) fn build_launch_bundle_anthropic_falls_back_to_pi_when_claude_missing() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi"]);
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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
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
        Some("pi")
    );
}

pub(crate) fn build_launch_bundle_anthropic_falls_back_to_pi_when_claude_auth_fails() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses_with_auth_failures(&temp, &["claude", "pi"], &["claude"]);
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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi")
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
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi")
    );
}

pub(crate) fn build_launch_bundle_builtin_gemini_model_alias_resolves_to_google_model_and_pi_harness()
 {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "gemini"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gemini-2.5-pro",
            "provider": "Google",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "gemini",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gemini"));
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gemini-2.5-pro"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_ne!(bundle["routing"]["harness"].as_str(), Some("gemini"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("constrained")
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
        Some("pi,opencode")
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
        Some("pi")
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
        Some("pi,opencode")
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
        Some("pi,opencode,cursor")
    );
}

pub(crate) fn build_launch_bundle_openai_falls_back_to_cursor_when_only_cursor_installed() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode,cursor")
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
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode")
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

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("pi,opencode,cursor")
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
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("opencode,pi")
    );
}

pub(crate) fn build_launch_bundle_legacy_harness_link_filters_ambient_path_candidates() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5.4-mini
---
Review code changes."#;

    let extra_toml = r#"[settings]
targets = [".opencode", ".agents"]"#;

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
        Some("provider")
    );
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("confirmed")
    );
    assert_eq!(
        bundle["provenance"]["candidates_tried"].as_str(),
        Some("opencode")
    );
}

pub(crate) fn build_launch_bundle_link_constraints_block_unrelated_default_fallbacks() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &[]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
---
Review code changes."#;

    let extra_toml = r#"[settings]
targets = [".claude"]
default_harness = "pi""#;

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
        Some("provider")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("settings.default_harness is excluded by known linked harness constraints")
    }));
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
    assert_eq!(
        bundle["routing"]["route_confidence"].as_str(),
        Some("passthrough")
    );
    assert_eq!(
        bundle["provenance"]["route_confidence"].as_str(),
        Some("passthrough")
    );
}

pub(crate) fn build_launch_bundle_synthesizes_opencode_model_when_cache_missing() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
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
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("openai/gpt-5")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("cached-probe")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("confirmed")
    );

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(
        warnings.is_empty(),
        "explicit harness passthrough path is intentional and should stay quiet: {warnings:?}"
    );
}

pub(crate) fn build_launch_bundle_explicit_unknown_harness_model_path_fails_closed() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
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
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().failure().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cli harness `opencode` cannot run requested model"));
    assert!(stderr.contains("no_model_match"));
}

pub(crate) fn build_launch_bundle_alias_fixed_native_harness_rejects_mismatched_provider_constraint()
 {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: badnative
---
Review code changes."#;

    let extra_toml = r#"[models.badnative]
model = "gpt-5"
provider = "anthropic"
harness = "codex""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().failure().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("alias harness `codex` cannot run requested model"));
    assert!(stderr.contains("provider_constraint_unsatisfied"));
}

pub(crate) fn build_launch_bundle_overlay_model_overrides_profile_model() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let extra_toml = r#"[models.gpt55]
model = "gpt-5"
harness = "codex"

[agents.reviewer]
model = "gpt55""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gpt55"));
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(
        bundle["provenance"]["model_source"].as_str(),
        Some("overlay")
    );
}

pub(crate) fn build_launch_bundle_settings_model_policy_applies_with_provenance() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt55
---
Review code changes."#;

    let extra_toml = r#"[models.gpt55]
model = "gpt-5"

[[settings.model-policies]]
match = { alias = "gpt55" }
override = { harness = "opencode", effort = "medium" }"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("opencode"));
    assert_eq!(
        bundle["execution_policy"]["effort"].as_str(),
        Some("medium")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("settings-model-policy")
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("settings-model-policy")
    );
    assert_eq!(
        bundle["provenance"]["matched_policy_rule"].as_str(),
        Some("settings:0")
    );
}

pub(crate) fn build_launch_bundle_composed_model_policies_overlay_wins() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["pi", "codex", "opencode"]);
    let agent_content = r#"---
name: reviewer
model: gpt55
model-policies:
  - match:
      alias: gpt55
    override:
      harness: codex
      effort: high
---
Review code changes."#;

    let extra_toml = r#"[models.gpt55]
model = "gpt-5"

[agents.reviewer]

[[agents.reviewer.model-policies]]
match = { alias = "gpt55" }
override = { harness = "pi", effort = "medium" }

[[settings.model-policies]]
match = { alias = "gpt55" }
override = { harness = "opencode", effort = "low" }"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["execution_policy"]["effort"].as_str(),
        Some("medium")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("overlay-model-policy")
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("overlay-model-policy")
    );
    assert_eq!(
        bundle["provenance"]["matched_policy_rule"].as_str(),
        Some("overlay:0")
    );
}

pub(crate) fn build_launch_bundle_composed_model_policies_first_match_wins() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "pi"]);
    let agent_content = r#"---
name: reviewer
model: gpt55
model-policies:
  - match:
      alias: gpt55
    override:
      harness: codex
---
Review code changes."#;

    let extra_toml = r#"[models.gpt55]
model = "gpt-5"

[agents.reviewer]

[[agents.reviewer.model-policies]]
match = { alias = "not-gpt55" }
override = { harness = "pi" }"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("profile-model-policy")
    );
    assert_eq!(
        bundle["provenance"]["matched_policy_rule"].as_str(),
        Some("profile:0")
    );
}

pub(crate) fn build_launch_bundle_local_overlay_replaces_base_overlay_by_name() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex", "pi"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let extra_toml = r#"[models.gpt55]
model = "gpt-5"
harness = "codex"

[models.gptmini]
model = "gpt-5.4-mini"
harness = "pi"

[agents.reviewer]
model = "gpt55"
harness = "codex"
effort = "high""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);
    fs::write(
        project_root.join("mars.local.toml"),
        r#"[agents.reviewer]
model = "gptmini""#,
    )
    .unwrap();

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gptmini"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["model_source"].as_str(),
        Some("overlay")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
}

fn install_fake_harnesses(temp: &TempDir, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp.path().join("harness-bin");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        #[cfg(windows)]
        {
            let script = if *harness == "pi" {
                "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo pi 0.0.0-test\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--help\" (\r\n  echo --mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--list-models\" (\r\n  echo openai gpt-5\r\n  echo openai gpt-5.4-mini\r\n  echo openai gpt-5.5\r\n  echo anthropic claude-opus-4-6\r\n  echo anthropic claude-opus-4-7\r\n  echo google gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else if *harness == "opencode" {
                "@echo off\r\nif \"%~1\"==\"models\" (\r\n  echo openai/gpt-5\r\n  echo openai/gpt-5.4-mini\r\n  echo openai/gpt-5.5\r\n  echo anthropic/claude-opus-4-6\r\n  echo anthropic/claude-opus-4-7\r\n  echo google/gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else {
                "@echo off\r\nexit /b 0\r\n"
            };
            fs::write(bin_dir.join(format!("{harness}.bat")), script).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin_dir.join(harness);
            let script = if *harness == "pi" {
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo \"pi 0.0.0-test\"\n  exit 0\nfi\nif [ \"$1\" = \"--help\" ]; then\n  echo \"--mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\"\n  exit 0\nfi\nif [ \"$1\" = \"--list-models\" ]; then\n  printf '%s\\n' \\\n    'openai gpt-5' \\\n    'openai gpt-5.4-mini' \\\n    'openai gpt-5.5' \\\n    'anthropic claude-opus-4-6' \\\n    'anthropic claude-opus-4-7' \\\n    'google gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else if *harness == "opencode" {
                "#!/bin/sh\nif [ \"$1\" = \"models\" ]; then\n  printf '%s\\n' \\\n    'openai/gpt-5' \\\n    'openai/gpt-5.4-mini' \\\n    'openai/gpt-5.5' \\\n    'anthropic/claude-opus-4-6' \\\n    'anthropic/claude-opus-4-7' \\\n    'google/gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else {
                "#!/bin/sh\nexit 0\n"
            };
            fs::write(&path, script).unwrap();
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
            let script = if *harness == "pi" {
                "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo pi 0.0.0-test\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--help\" (\r\n  echo --mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--list-models\" (\r\n  echo openai gpt-5\r\n  echo openai gpt-5.4-mini\r\n  echo openai gpt-5.5\r\n  echo anthropic claude-opus-4-6\r\n  echo anthropic claude-opus-4-7\r\n  echo google gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else if *harness == "opencode" {
                "@echo off\r\nif \"%~1\"==\"models\" (\r\n  echo openai/gpt-5\r\n  echo openai/gpt-5.4-mini\r\n  echo openai/gpt-5.5\r\n  echo anthropic/claude-opus-4-6\r\n  echo anthropic/claude-opus-4-7\r\n  echo google/gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else if fail_auth && *harness == "codex" {
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
            let script = if *harness == "pi" {
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo \"pi 0.0.0-test\"\n  exit 0\nfi\nif [ \"$1\" = \"--help\" ]; then\n  echo \"--mode rpc --model --append-system-prompt --session --fork --session-dir PI_CODING_AGENT_SESSION_DIR --no-extensions --no-skills --no-context-files --no-prompt-templates -e\"\n  exit 0\nfi\nif [ \"$1\" = \"--list-models\" ]; then\n  printf '%s\\n' \\\n    'openai gpt-5' \\\n    'openai gpt-5.4-mini' \\\n    'openai gpt-5.5' \\\n    'anthropic claude-opus-4-6' \\\n    'anthropic claude-opus-4-7' \\\n    'google gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else if *harness == "opencode" {
                "#!/bin/sh\nif [ \"$1\" = \"models\" ]; then\n  printf '%s\\n' \\\n    'openai/gpt-5' \\\n    'openai/gpt-5.4-mini' \\\n    'openai/gpt-5.5' \\\n    'anthropic/claude-opus-4-6' \\\n    'anthropic/claude-opus-4-7' \\\n    'google/gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
            } else if fail_auth && *harness == "codex" {
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
