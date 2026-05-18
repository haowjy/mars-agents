// qa-validated: capability-cache-resolver-invalid-harness-packages

mod common;

use httpmock::prelude::*;
use serde_json::{Value, json};
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use common::*;

#[test]
#[serial]
fn scenario_f_add_sync_force_and_resolve_dependency_alias() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let (temp, project_root) = setup_project(&server);
    let source_root = write_local_source_with_model_alias(
        temp.path(),
        "alias-source-force-sync",
        "test-alias",
        "openai/gpt-5",
    );

    let mut add_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    add_cmd.arg("add").arg(source_root.as_os_str());
    add_cmd.assert().success();

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["sync", "--force"]);
    cmd.assert().success();

    let mut resolve_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    resolve_cmd.args(["--json", "models", "resolve", "test-alias"]);
    let resolve_output = resolve_cmd.assert().success().get_output().clone();
    let resolve_json: Value =
        serde_json::from_slice(&resolve_output.stdout).expect("resolve --json should return JSON");
    assert_eq!(
        resolve_json["resolved_model"].as_str(),
        Some("openai/gpt-5"),
        "expected dependency alias to resolve to pinned model"
    );

    let cache = read_cache_json(&project_root);
    assert!(
        cache["models"]
            .as_array()
            .expect("cache.models should be an array")
            .len()
            >= 2,
        "expected sync to populate models cache"
    );
    assert!(
        cache["fetched_at"].as_str().is_some(),
        "expected fetched_at to be set after sync"
    );
    assert!(
        models_merged_path(&project_root).exists(),
        "expected models-merged.json to be written during sync"
    );
    let merged: Value = serde_json::from_str(
        &fs::read_to_string(models_merged_path(&project_root))
            .expect("failed to read models-merged.json"),
    )
    .expect("failed to parse models-merged.json");
    assert!(
        merged.get("test-alias").is_some(),
        "expected dependency alias in models-merged.json"
    );
    assert_eq!(
        mock.hits(),
        1,
        "expected add+sync+resolve flow to fetch models catalog once"
    );
}

#[test]
#[serial]
fn scenario_h_add_immediately_resolve_alias_without_explicit_sync() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let (temp, project_root) = setup_project(&server);
    let source_root = write_local_source_with_model_alias(
        temp.path(),
        "alias-source-immediate",
        "test-alias-immediate",
        "openai/gpt-5",
    );

    let mut add_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    add_cmd.arg("add").arg(source_root.as_os_str());
    add_cmd.assert().success();

    let mut resolve_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    resolve_cmd.args(["models", "resolve", "test-alias-immediate"]);
    let resolve_output = resolve_cmd.assert().success().get_output().clone();
    let resolve_stdout =
        String::from_utf8(resolve_output.stdout).expect("resolve stdout should be utf-8");
    assert!(
        resolve_stdout.contains("openai/gpt-5"),
        "expected resolved pinned model in resolve output:\n{resolve_stdout}"
    );
    assert!(
        models_merged_path(&project_root).exists(),
        "expected models-merged.json after add-triggered sync"
    );
    assert_eq!(
        mock.hits(),
        1,
        "expected add+immediate resolve online flow to fetch models catalog once"
    );
}

#[test]
#[serial]
fn resolve_alias_prefix_exits_zero() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "opus-4-6"]);

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("alias_prefix"));
    assert_eq!(stdout["name"].as_str(), Some("opus-4-6"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("claude-opus-4-6"));
}

#[test]
#[serial]
fn resolve_unknown_exits_zero_with_passthrough() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &[]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "unknown-xyz"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["model_id"].as_str(), Some("unknown-xyz"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("unknown-xyz"));
    assert_eq!(stdout["provider"], Value::Null);
    assert_eq!(stdout["harness"], Value::Null);
    assert_eq!(stdout["harness_source"].as_str(), Some("unavailable"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["pi", "opencode", "cursor"])
    );
    assert!(stdout["availability"].is_string());
    assert!(stdout["availability_source"].is_string());
    assert!(
        stdout["runnable_paths"].is_array(),
        "passthrough JSON should include availability runnable paths"
    );
    assert!(
        stdout["warning"]
            .as_str()
            .expect("passthrough warning should be present")
            .contains("passing through to harness")
    );
}

#[test]
#[serial]
fn models_list_visibility_include_does_not_add_catalog_rows() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[settings.model_visibility]
include = ["catalog-only-*"]
"#,
    )
    .expect("failed to write mars.toml with model visibility");
    write_cache(
        &project_root,
        vec![json!({
            "id": "catalog-only-model",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "list"]);

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("models list --json should return JSON");

    let aliases = stdout["aliases"]
        .as_array()
        .expect("models list JSON should include aliases");
    assert!(
        aliases.is_empty(),
        "default models list should not expand visibility includes into catalog rows"
    );
}

#[test]
#[serial]
fn resolve_prefix_no_match_exits_zero_with_passthrough() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "opus-9-9"]);

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("opus-9-9"));
}

#[test]
#[serial]
fn resolve_passthrough_pattern_guesses_harness() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "claude-brand-new"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider"].as_str(), Some("anthropic"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["claude", "pi", "opencode", "cursor"])
    );
    assert_eq!(stdout["harness_source"].as_str(), Some("pattern_guess"));
    assert_eq!(stdout["harness"].as_str(), Some("pi"));
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_harness_null() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &[]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider"], Value::Null);
    assert_eq!(stdout["harness"], Value::Null);
    assert_eq!(stdout["harness_source"].as_str(), Some("unavailable"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["pi", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_uses_unknown_provider_fallback_harnesses() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider"], Value::Null);
    assert_eq!(stdout["harness"].as_str(), Some("pi"));
    assert_eq!(stdout["harness_source"].as_str(), Some("pattern_guess"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["pi", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_opencode_only_is_unknown_availability() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());
    write_opencode_probe_cache(temp.path(), json!({"openai": true}), vec!["openai/gpt-5"]);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider"], Value::Null);
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["harness_source"].as_str(), Some("pattern_guess"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["pi", "opencode", "cursor"])
    );
    assert_eq!(stdout["availability"].as_str(), Some("unknown"));
    assert_eq!(
        stdout["availability_source"].as_str(),
        Some("opencode_probe_unknown")
    );
}

#[test]
#[serial]
fn resolve_builtin_gemini_alias_uses_google_candidates_without_gemini_harness() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "gemini"]);
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
    cmd.args(["--json", "models", "resolve", "gemini"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["name"].as_str(), Some("gemini"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("gemini-2.5-pro"));
    assert_eq!(stdout["provider"].as_str(), Some("google"));
    assert_eq!(stdout["harness"].as_str(), Some("pi"));
    assert_ne!(stdout["harness"].as_str(), Some("gemini"));
    assert_eq!(
        stdout["harness_candidates"],
        json!(["pi", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn sync_rejects_dependency_model_alias_with_invalid_harness() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let source_root = temp.path().join("invalid-harness-source");
    fs::create_dir_all(source_root.join("agents")).expect("failed to create source agents dir");
    fs::write(
        source_root.join("agents").join("fixture.md"),
        "# Fixture agent for invalid harness package test\n",
    )
    .expect("failed to write source agent");
    fs::write(
        source_root.join("mars.toml"),
        r#"[package]
name = "invalid-harness-source"
version = "0.1.0"

[models.bad]
harness = "gemini"
model = "gemini-2.5-pro"
"#,
    )
    .expect("failed to write source manifest");
    fs::write(
        project_root.join("mars.toml"),
        format!(
            "[dependencies]\ninvalid_harness_source = {{ path = \"{}\" }}\n",
            source_root.display().to_string().replace('\\', "/")
        ),
    )
    .expect("failed to write project manifest");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.arg("sync");

    let output = cmd.assert().failure().get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("invalid harness 'gemini'"),
        "expected invalid harness diagnostic, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("valid harnesses: claude, codex, pi, opencode, cursor"),
        "expected valid harness list, stderr:\n{stderr}"
    );
}

#[test]
#[serial]
fn resolve_unknown_with_no_refresh_without_cache_is_non_zero() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["models", "resolve", "unknown-xyz", "--no-refresh-models"]);

    let output = cmd.assert().code(3).get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("--no-refresh-models"),
        "expected no-refresh cache error, stderr:\n{stderr}"
    );
}

fn install_fake_harnesses(temp_root: &Path, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp_root.join("harness-bin");
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

fn replace_path_with(bin_dir: &Path) -> String {
    bin_dir.to_string_lossy().into_owned()
}

fn write_opencode_probe_cache(temp_root: &Path, providers: Value, model_slugs: Vec<&str>) {
    let cache_dir = temp_root
        .join("xdg-cache")
        .join("mars")
        .join("cache")
        .join("availability");
    fs::create_dir_all(&cache_dir).expect("failed to create probe cache dir");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_secs();
    let payload = json!({
        "schema_version": 1,
        "fetched_at": now,
        "last_attempt_at": now,
        "last_error": Value::Null,
        "result": {
            "providers": providers,
            "model_slugs": model_slugs,
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": Value::Null
        }
    });
    fs::write(
        cache_dir.join("opencode-probe.json"),
        serde_json::to_vec_pretty(&payload).expect("failed to serialize probe cache payload"),
    )
    .expect("failed to write opencode probe cache");
}
