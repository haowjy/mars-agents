use super::common::setup_bundle_project;
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::{Value, json};
use std::fs;
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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("openai_alias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
}

pub(crate) fn build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness()
{
    let temp = TempDir::new().unwrap();
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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
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
}

pub(crate) fn build_launch_bundle_uses_alias_provider_when_auto_resolve_misses_model_cache() {
    let temp = TempDir::new().unwrap();
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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
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

pub(crate) fn build_launch_bundle_resolves_harness_model_from_cached_opencode_probe() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;
    let extra_toml = r#"[models.gpt55]
model = "gpt-5.5""#;

    write_opencode_probe_cache(
        &temp,
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

pub(crate) fn build_launch_bundle_ignores_stale_opencode_probe_and_synthesizes_runnable_path() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: gpt55
---
Review code changes."#;
    let extra_toml = r#"[models.gpt55]
model = "gpt-5.5""#;

    write_opencode_probe_cache(
        &temp,
        1,
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
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "opencode",
    ]);
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

pub(crate) fn build_launch_bundle_synthesizes_openrouter_path_when_probe_has_openrouter_only() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: opus
---
Review code changes."#;
    let extra_toml = r#"[models.opus]
model = "claude-opus-4-7""#;

    write_opencode_probe_cache(
        &temp,
        now_unix_secs(),
        json!({
            "providers": { "openrouter": true },
            "model_slugs": ["openrouter/anthropic/claude-sonnet-4-7"],
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
        "--harness",
        "opencode",
    ]);
    cmd.env("MARS_PROBE_CACHE_TTL_SECS", "60");

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["harness_model"].as_str(),
        Some("openrouter/anthropic/claude-opus-4-7")
    );
    assert_eq!(
        bundle["routing"]["harness_model_source"].as_str(),
        Some("synthesized")
    );
    assert_eq!(
        bundle["routing"]["harness_model_confidence"].as_str(),
        Some("likely")
    );
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

fn write_opencode_probe_cache(temp: &TempDir, fetched_at: u64, result: Value) {
    let cache_path = temp
        .path()
        .join("xdg-cache")
        .join("mars")
        .join("cache")
        .join("availability")
        .join("opencode-probe.json");
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
