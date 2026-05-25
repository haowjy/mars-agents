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
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
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
    resolve_cmd.env("PATH", replace_path_with(&bin_dir));
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
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
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
    resolve_cmd.env("PATH", replace_path_with(&bin_dir));
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
fn resolve_unknown_fails_cleanly_when_no_harness_reports_model_slug() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &[]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "unknown-xyz"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["model_id"].as_str(), Some("unknown-xyz"));
    assert!(
        stdout["error"]
            .as_str()
            .expect("error should be present")
            .contains("selected harness 'pi', but that harness is not installed")
    );
    assert_eq!(
        stdout["route_rejection"]["reason"].as_str(),
        Some("harness_not_installed")
    );
    assert_eq!(stdout["route_rejection"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        stdout["harnesses_tried"],
        json!(["claude", "pi", "codex", "opencode", "cursor"])
    );
    assert!(stdout["route_trace"].is_object());
    assert_eq!(stdout["route_trace"]["version"].as_u64(), Some(1));
    let assessments = stdout["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    let pi_assessment = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("pi"))
        .expect("pi assessment should exist");
    assert_eq!(pi_assessment["skip_reason"].as_str(), Some("not_installed"));
    assert_eq!(stdout["route"]["harness"].as_str(), Some("pi"));
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
fn models_list_uses_local_model_visibility_overlay() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[settings.model_visibility]
include = ["gpt-5"]

[models.fast]
harness = "codex"
model = "gpt-5"
provider = "openai"

[models.slow]
harness = "codex"
model = "gpt-5.4-mini"
provider = "openai"
"#,
    )
    .expect("failed to write mars.toml");
    fs::write(
        project_root.join("mars.local.toml"),
        r#"[settings.model_visibility]
include = ["gpt-5.4-mini"]
"#,
    )
    .expect("failed to write mars.local.toml");
    write_cache(
        &project_root,
        vec![
            json!({
                "id": "gpt-5",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
            json!({
                "id": "gpt-5.4-mini",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
        ],
        &fresh_fetched_at(),
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "list", "--unavailable"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("models list --json should return JSON");
    let aliases = stdout["aliases"]
        .as_array()
        .expect("models list JSON should include aliases");
    let names: Vec<_> = aliases
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();

    assert_eq!(names, vec!["slow"]);
}

#[test]
#[serial]
fn resolve_prefix_no_match_fails_cleanly() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "opus-9-9"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("opus-9-9"));
    assert!(
        stdout["error"]
            .as_str()
            .expect("error should be present")
            .contains("did not match any harness-reported model slug")
    );
}

#[test]
#[serial]
fn resolve_passthrough_pattern_without_match_fails_cleanly() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "claude-brand-new"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("claude-brand-new"));
    assert_eq!(
        stdout["harnesses_tried"],
        json!(["claude", "pi", "codex", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_with_no_harnesses_fails_cleanly() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &[]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider_constraint"], Value::Null);
    assert_eq!(
        stdout["harnesses_tried"],
        json!(["claude", "pi", "codex", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_with_pi_installed_still_fails_closed() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider_constraint"], Value::Null);
    assert_eq!(
        stdout["harnesses_tried"],
        json!(["claude", "pi", "codex", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_unrecognized_pattern_opencode_only_fails_closed() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());
    write_opencode_probe_cache(temp.path(), json!({"openai": true}), vec!["openai/gpt-5"]);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "xyz-unknown"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["provider_constraint"], Value::Null);
    assert_eq!(
        stdout["harnesses_tried"],
        json!(["claude", "pi", "codex", "opencode", "cursor"])
    );
}

#[test]
#[serial]
fn resolve_passthrough_provider_model_slug_succeeds_when_harness_reports_match() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());
    write_opencode_probe_cache(
        temp.path(),
        json!({"openai": true}),
        vec!["openai/gpt-5.4-mini"],
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "openai/gpt-5.4-mini"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["model_id"].as_str(), Some("gpt-5.4-mini"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("gpt-5.4-mini"));
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["harness_source"].as_str(), Some("pattern_guess"));
    assert!(
        stdout.get("warning").is_none(),
        "confirmed passthrough route should not emit warning: {stdout}"
    );
    assert!(stdout["route_trace"].is_object());
    let assessments = stdout["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    assert!(
        assessments.iter().any(|assessment| {
            assessment["chosen_slug"].as_str() == Some("openai/gpt-5.4-mini")
        })
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
fn models_list_exact_alias_respects_settings_harness_order() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "cursor"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]
harness_order = ["cursor", "pi"]

[models.fast]
model = "gpt-5.4-mini"
"#,
    )
    .expect("failed to write mars.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5.4-mini",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );
    write_cursor_probe_cache(temp.path(), vec!["gpt-5.4-mini"]);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "list"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("models list --json should return JSON");
    let aliases = stdout["aliases"]
        .as_array()
        .expect("models list JSON should include aliases");
    let fast = aliases
        .iter()
        .find(|entry| entry["name"].as_str() == Some("fast"))
        .expect("expected fast alias entry");

    assert_eq!(fast["harness"].as_str(), Some("cursor"));
    assert_eq!(fast["harness_source"].as_str(), Some("auto_detected"));
}

#[test]
#[serial]
fn models_list_exact_alias_respects_local_harness_order_override() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "cursor"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]
harness_order = ["pi", "cursor"]

[models.fast]
model = "gpt-5.4-mini"
"#,
    )
    .expect("failed to write mars.toml");
    fs::write(
        project_root.join("mars.local.toml"),
        r#"[settings]
harness_order = ["cursor", "pi"]
"#,
    )
    .expect("failed to write mars.local.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5.4-mini",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );
    write_cursor_probe_cache(temp.path(), vec!["gpt-5.4-mini"]);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "list"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("models list --json should return JSON");
    let aliases = stdout["aliases"]
        .as_array()
        .expect("models list JSON should include aliases");
    let fast = aliases
        .iter()
        .find(|entry| entry["name"].as_str() == Some("fast"))
        .expect("expected fast alias entry");

    assert_eq!(fast["harness"].as_str(), Some("cursor"));
    assert_eq!(fast["harness_source"].as_str(), Some("auto_detected"));
}

#[test]
#[serial]
fn resolve_exact_alias_respects_settings_harness_order() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "cursor"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]
harness_order = ["cursor", "pi"]

[models.fast]
model = "gpt-5.4-mini"
"#,
    )
    .expect("failed to write mars.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5.4-mini",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );
    write_cursor_probe_cache(temp.path(), vec!["gpt-5.4-mini"]);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "fast"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["harness"].as_str(), Some("cursor"));
    assert_eq!(stdout["harness_source"].as_str(), Some("auto_detected"));
}

#[test]
#[serial]
fn resolve_exact_alias_uses_local_model_overlay_replace_by_key() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[models.fast]
harness = "codex"
model = "gpt-5"
provider = "openai"
"#,
    )
    .expect("failed to write mars.toml");
    fs::write(
        project_root.join("mars.local.toml"),
        r#"[models.fast]
harness = "pi"
model = "gpt-5.4-mini"
provider = "openai"
"#,
    )
    .expect("failed to write mars.local.toml");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "fast", "--no-refresh-models"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(
        stdout["source"].as_str(),
        Some("consumer local (mars.local.toml)")
    );
    assert_eq!(stdout["resolved_model"].as_str(), Some("gpt-5.4-mini"));
    assert_eq!(stdout["harness"].as_str(), Some("pi"));
    assert_eq!(stdout["spec"]["model"].as_str(), Some("gpt-5.4-mini"));
}

#[test]
#[serial]
fn resolve_gpt5_prefers_codex_over_deferred_pi_passthrough() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]
harness_order = ["codex", "pi"]
"#,
    )
    .expect("failed to write mars.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    for model_arg in ["gpt-5", "openai/gpt-5"] {
        let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
        cmd.args(["--json", "models", "resolve", model_arg]);
        cmd.env("PATH", replace_path_with(&bin_dir));

        let output = cmd.assert().success().get_output().clone();
        let stdout: Value =
            serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

        assert_eq!(
            stdout["harness"].as_str(),
            Some("codex"),
            "models resolve {model_arg} should match build routing to codex"
        );
        assert!(
            matches!(
                stdout["route"]["match_evidence"].as_str(),
                Some("confirmed") | Some("constrained")
            ),
            "unexpected match evidence for {model_arg}: {}",
            stdout["route"]["match_evidence"]
        );
        assert_eq!(stdout["route"]["source"].as_str(), Some("config-order"));
    }
}

#[test]
#[serial]
fn resolve_raw_model_uses_local_harness_order_overlay_and_suppresses_catalog_warning() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["pi", "codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]
harness_order = ["pi", "codex"]
"#,
    )
    .expect("failed to write mars.toml");
    fs::write(
        project_root.join("mars.local.toml"),
        r#"[settings]
harness_order = ["codex", "pi"]
"#,
    )
    .expect("failed to write mars.local.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    for model_arg in ["gpt-5", "openai/gpt-5"] {
        let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
        cmd.args(["--json", "models", "resolve", model_arg]);
        cmd.env("PATH", replace_path_with(&bin_dir));

        let output = cmd.assert().success().get_output().clone();
        let stdout: Value =
            serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

        assert_eq!(stdout["harness"].as_str(), Some("codex"));
        assert_eq!(stdout["route"]["source"].as_str(), Some("config-order"));
        assert_eq!(
            stdout["route_trace"]["candidates_tried"],
            json!(["codex"]),
            "local harness_order should replace project harness order for {model_arg}"
        );
        assert!(
            stdout.get("warning").is_none(),
            "confirmed/constrained passthrough resolves should not emit catalog passthrough warning: {stdout}"
        );
    }

    let mut text_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    text_cmd.args(["models", "resolve", "gpt-5"]);
    text_cmd.env("PATH", replace_path_with(&bin_dir));
    let output = text_cmd.assert().success().get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        !stderr.contains("not found in catalog, passing through to harness"),
        "text output should suppress passthrough catalog warning when route evidence is confirmed: {stderr}"
    );
}

#[test]
#[serial]
fn resolve_exact_alias_fixed_native_harness_fails_when_provider_constraint_is_incompatible() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[models.badnative]
harness = "codex"
model = "gpt-5"
provider = "anthropic"
"#,
    )
    .expect("failed to write mars.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "badnative"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert!(
        stdout["error"]
            .as_str()
            .expect("error should be present")
            .contains("cannot run resolved model under model-first routing")
    );
    assert_eq!(stdout["route"]["harness"].as_str(), Some("codex"));
    assert_eq!(stdout["route"]["selection_kind"].as_str(), Some("fixed"));
    assert_eq!(stdout["route"]["match_evidence"].as_str(), Some("none"));
    assert_eq!(
        stdout["route_rejection"]["reason"].as_str(),
        Some("assessment_failed")
    );
    assert_eq!(stdout["route_rejection"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        stdout["route_rejection"]["skip_reason"].as_str(),
        Some("provider_constraint_unsatisfied")
    );
    assert_eq!(stdout["route_trace"]["candidates_tried"], json!(["codex"]));
    assert_eq!(stdout["route_trace"]["version"].as_u64(), Some(1));
    assert_eq!(stdout["harnesses_tried"], json!(["codex"]));
    let assessments = stdout["route_trace"]["assessments"]
        .as_array()
        .expect("route_trace.assessments should be array");
    let codex_assessment = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("codex"))
        .expect("codex assessment should exist");
    assert_eq!(
        codex_assessment["skip_reason"].as_str(),
        Some("provider_constraint_unsatisfied")
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

#[test]
#[serial]
fn resolve_pinned_exact_alias_json_no_refresh_without_cache_succeeds() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[models.fast]
harness = "codex"
model = "gpt-5"
provider = "openai"
"#,
    )
    .expect("failed to write mars.toml");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "fast", "--no-refresh-models"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");
    assert_eq!(stdout["name"].as_str(), Some("fast"));
    assert_eq!(stdout["resolved_model"].as_str(), Some("gpt-5"));
    assert_eq!(stdout["harness"].as_str(), Some("codex"));
    assert_eq!(stdout["route"]["source"].as_str(), Some("alias"));
    assert_eq!(stdout["route"]["selection_kind"].as_str(), Some("fixed"));
    assert_eq!(
        stdout["route"]["match_evidence"].as_str(),
        Some("constrained")
    );
    assert_eq!(
        stdout["route_trace"]["selection_kind"].as_str(),
        Some("fixed")
    );
    assert_eq!(
        stdout["route_trace"]["match_evidence"].as_str(),
        Some("constrained")
    );
    assert_eq!(stdout["route_trace"]["version"].as_u64(), Some(1));
    assert!(
        stdout.get("route_rejection").is_none(),
        "successful exact alias resolves should not emit route_rejection: {stdout}"
    );
    assert!(
        stdout.get("cache_error").is_none(),
        "pinned exact aliases should not require a models cache: {stdout}"
    );
}

#[test]
#[serial]
fn resolve_auto_exact_alias_json_no_refresh_without_cache_returns_alias_cache_error() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[models.fast]
provider = "openai"
match = ["gpt-5*"]
"#,
    )
    .expect("failed to write mars.toml");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "fast", "--no-refresh-models"]);

    let output = cmd.assert().code(1).get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");
    assert_eq!(stdout["name"].as_str(), Some("fast"));
    assert_eq!(stdout["source"].as_str(), Some("consumer (mars.toml)"));
    let error = stdout["error"].as_str().expect("error should be present");
    assert!(
        error.starts_with("alias `fast` requires models cache for auto-resolve"),
        "expected alias-specific cache error, got: {error}"
    );
    assert!(
        stdout["cache_error"]
            .as_str()
            .expect("cache_error should be present")
            .contains("--no-refresh-models")
    );
    assert!(
        stdout.get("route").is_none(),
        "auto exact alias with unavailable cache must not fall through to passthrough routing: {stdout}"
    );
    assert!(
        stdout.get("route_trace").is_none(),
        "auto exact alias cache errors should not emit a mixed routing trace: {stdout}"
    );
    assert!(
        stdout.get("route_rejection").is_none(),
        "cache-unavailable alias errors should not masquerade as route rejections: {stdout}"
    );
}

#[test]
#[serial]
fn resolve_uses_pi_probe_compatibility_for_harness_routing() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    let bin_dir =
        install_fake_harnesses_with_pi_help(temp.path(), &["pi", "cursor"], "--mode rpc --model");
    fs::write(
        project_root.join("mars.toml"),
        r#"[settings]

[models.fast]
model = "gpt-5.4-mini"
"#,
    )
    .expect("failed to write mars.toml");
    write_cache(
        &project_root,
        vec![json!({
            "id": "gpt-5.4-mini",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "resolve", "fast"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let stdout: Value =
        serde_json::from_slice(&output.stdout).expect("resolve --json should return JSON");

    assert_eq!(stdout["harness"].as_str(), Some("cursor"));
    assert_eq!(stdout["harness_source"].as_str(), Some("auto_detected"));
}

#[test]
#[serial]
fn models_list_fails_when_local_settings_cannot_parse() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    fs::write(
        project_root.join("mars.local.toml"),
        "[settings]\nharness_order = 1\n",
    )
    .expect("failed to write invalid mars.local.toml");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "list"]);

    let output = cmd.assert().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("parse error"),
        "expected parse error for invalid local settings, stderr:\n{stderr}"
    );
}

#[test]
#[serial]
fn models_resolve_fails_when_local_settings_cannot_parse() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    fs::write(
        project_root.join("mars.local.toml"),
        "[settings]\nharness_order = 1\n",
    )
    .expect("failed to write invalid mars.local.toml");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["models", "resolve", "gpt-5"]);

    let output = cmd.assert().code(2).get_output().clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("parse error"),
        "expected parse error for invalid local settings, stderr:\n{stderr}"
    );
}

fn install_fake_harnesses(temp_root: &Path, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp_root.join("harness-bin");
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

fn install_fake_harnesses_with_pi_help(
    temp_root: &Path,
    harnesses: &[&str],
    pi_help_output: &str,
) -> PathBuf {
    let bin_dir = temp_root.join("harness-bin-pi-help");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        #[cfg(windows)]
        {
            let script = if *harness == "pi" {
                format!(
                    "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo pi 0.0.0-test\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--help\" (\r\n  echo {pi_help_output}\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"--list-models\" (\r\n  echo openai gpt-5\r\n  echo openai gpt-5.4-mini\r\n  echo openai gpt-5.5\r\n  echo anthropic claude-opus-4-6\r\n  echo anthropic claude-opus-4-7\r\n  echo google gemini-2.5-pro\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
                )
            } else {
                "@echo off\r\nexit /b 0\r\n".to_string()
            };
            fs::write(bin_dir.join(format!("{harness}.bat")), script).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin_dir.join(harness);
            let script = if *harness == "pi" {
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo \"pi 0.0.0-test\"\n  exit 0\nfi\nif [ \"$1\" = \"--help\" ]; then\n  echo \"{pi_help_output}\"\n  exit 0\nfi\nif [ \"$1\" = \"--list-models\" ]; then\n  printf '%s\\n' \\\n    'openai gpt-5' \\\n    'openai gpt-5.4-mini' \\\n    'openai gpt-5.5' \\\n    'anthropic claude-opus-4-6' \\\n    'anthropic claude-opus-4-7' \\\n    'google gemini-2.5-pro'\n  exit 0\nfi\nexit 0\n"
                )
            } else {
                "#!/bin/sh\nexit 0\n".to_string()
            };
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

fn write_cursor_probe_cache(temp_root: &Path, slugs: Vec<&str>) {
    let cache_dir = temp_root.join("mars-cache").join("availability");
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
            "slugs": slugs,
            "model_probe_success": true,
            "error": Value::Null
        }
    });
    fs::write(
        cache_dir.join("cursor-probe.json"),
        serde_json::to_vec_pretty(&payload).expect("failed to serialize probe cache payload"),
    )
    .expect("failed to write cursor probe cache");
}

fn write_opencode_probe_cache(temp_root: &Path, providers: Value, model_slugs: Vec<&str>) {
    let cache_dir = temp_root.join("mars-cache").join("availability");
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
