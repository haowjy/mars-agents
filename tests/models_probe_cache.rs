// qa-validated: capability-cache-resolver-live-probes

mod common;

use assert_cmd::Command;
use serde_json::{Value, json};
use serial_test::serial;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use common::fresh_fetched_at;

fn mars_cmd() -> Command {
    Command::cargo_bin("mars").unwrap()
}

fn write_mars_toml(dir: &Path, content: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("mars.toml"), content).unwrap();
}

fn write_models_cache(dir: &Path, models_json: &str) {
    let mars_dir = dir.join(".mars");
    fs::create_dir_all(&mars_dir).unwrap();
    fs::write(mars_dir.join("models-cache.json"), models_json).unwrap();
}

fn write_probe_cache(cache_dir: &Path, entry_json: &str) {
    let avail_dir = cache_dir.join("availability");
    fs::create_dir_all(&avail_dir).unwrap();
    fs::write(avail_dir.join("opencode-probe.json"), entry_json).unwrap();
}

fn models_cache_json() -> String {
    serde_json::to_string_pretty(&json!({
        "models": [{
            "id": "gpt-5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        }],
        "fetched_at": fresh_fetched_at()
    }))
    .unwrap()
}

fn probe_cache_json(fetched_at: u64, model_slugs: &[&str]) -> String {
    serde_json::to_string_pretty(&json!({
        "schema_version": 1,
        "fetched_at": fetched_at,
        "last_attempt_at": fetched_at,
        "last_error": null,
        "result": {
            "model_slugs": model_slugs,
            "model_probe_success": true,
            "error": null
        }
    }))
    .unwrap()
}

fn setup_alias_project(project_root: &Path) {
    write_mars_toml(
        project_root,
        r#"[settings]

[models.fast]
harness = "opencode"
model = "gpt-5"
"#,
    );
    write_models_cache(project_root, &models_cache_json());
}

fn setup_raw_route_project(project_root: &Path) {
    write_mars_toml(
        project_root,
        r#"[settings]
harness_order = ["opencode", "codex"]
"#,
    );
    write_models_cache(project_root, &models_cache_json());
}

fn setup_alias_prefix_project(project_root: &Path) {
    write_mars_toml(
        project_root,
        r#"[settings]

[models.fast]
provider = "openai"
match = ["gpt-5*"]
"#,
    );
    write_models_cache(project_root, &models_cache_json());
}

fn install_fake_opencode(bin_dir: &Path, model_slugs: &[&str], marker: Option<&Path>) {
    #[cfg(windows)]
    {
        let marker_line = marker
            .map(|path| format!("  echo ran>>\"{}\"\r\n", path.display()))
            .unwrap_or_default();
        let mut script = String::from("@echo off\r\nif \"%~1\"==\"models\" (\r\n");
        script.push_str(&marker_line);
        for slug in model_slugs {
            script.push_str(&format!("  echo {slug}\r\n"));
        }
        script.push_str("  exit /b 0\r\n)\r\nexit /b 0\r\n");
        fs::write(bin_dir.join("opencode.bat"), script).unwrap();
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut script = String::from("#!/bin/sh\nif [ \"$1\" = \"models\" ]; then\n");
        if let Some(path) = marker {
            script.push_str(&format!("  printf 'ran\\n' >> '{}'\n", path.display()));
        }
        for slug in model_slugs {
            script.push_str(&format!("  printf '%s\\n' '{slug}'\n"));
        }
        script.push_str("  exit 0\nfi\nexit 0\n");
        let path = bin_dir.join("opencode");
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
}

fn install_fake_codex(bin_dir: &Path) {
    #[cfg(windows)]
    {
        fs::write(
            bin_dir.join("codex.bat"),
            "@echo off\r\nif \"%~1 %~2\"==\"login status\" exit /b 0\r\nexit /b 0\r\n",
        )
        .unwrap();
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("codex");
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$1\" = \"login\" ] && [ \"$2\" = \"status\" ]; then\n  exit 0\nfi\nexit 0\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
}

fn prepend_path(bin_dir: &Path) -> String {
    bin_dir.to_string_lossy().into_owned()
}

#[test]
#[serial]
fn refresh_models_ignores_prepopulated_probe_cache_and_uses_live_probe() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_alias_project(&project_root);
    install_fake_opencode(&bin_dir, &["openai/gpt-5"], None);

    let stale_cache = probe_cache_json(1, &["openai/not-gpt-5"]);
    write_probe_cache(&cache_dir, &stale_cache);

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast", "--refresh-models"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("MARS_PROBE_CACHE_TTL_SECS", "60")
        .env("PATH", prepend_path(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["resolved_model"].as_str(), Some("gpt-5"));
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["probe_cache"].as_str(), Some("miss"));
    assert_eq!(stdout["availability"].as_str(), Some("runnable"));
    assert_eq!(
        stdout["availability_source"].as_str(),
        Some("opencode_probe")
    );
    assert_eq!(
        stdout["runnable_paths"],
        json!([{
            "harness": "opencode",
            "mars_provider": "openai",
            "harness_model_id": "openai/gpt-5"
        }])
    );
}

#[test]
#[serial]
fn no_refresh_models_skips_live_probe_even_when_stale_cache_exists() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = temp.path().join("bin");
    let marker = temp.path().join("opencode-probe-ran");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_alias_project(&project_root);
    install_fake_opencode(&bin_dir, &["openai/gpt-5"], Some(&marker));

    let stale_cache = probe_cache_json(1, &["openai/gpt-5"]);
    write_probe_cache(&cache_dir, &stale_cache);

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast", "--no-refresh-models"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["resolved_model"].as_str(), Some("gpt-5"));
    assert_eq!(stdout["probe_cache"].as_str(), Some("stale"));
    assert!(
        !marker.exists(),
        "--no-refresh-models should skip command-scoped probe execution"
    );
}

#[test]
#[serial]
fn resolve_raw_model_uses_stale_probe_cache_for_route_selection_by_default() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_raw_route_project(&project_root);
    install_fake_opencode(&bin_dir, &["openai/not-gpt-5"], None);
    install_fake_codex(&bin_dir);

    write_probe_cache(&cache_dir, &probe_cache_json(1, &["openai/gpt-5"]));

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "gpt-5"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("MARS_PROBE_CACHE_TTL_SECS", "60")
        .env("PATH", prepend_path(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["probe_cache"].as_str(), Some("stale"));
    assert_eq!(stdout["route"]["source"].as_str(), Some("config-order"));
    let assessments = stdout["route_trace"]["assessments"]
        .as_array()
        .expect("route assessments should be array");
    let opencode = assessments
        .iter()
        .find(|assessment| assessment["harness"].as_str() == Some("opencode"))
        .expect("opencode assessment should exist");
    assert_eq!(opencode["skip_reason"], Value::Null);
}

#[test]
#[serial]
fn resolve_alias_prefix_uses_loaded_live_probe_for_runnable_availability() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    setup_alias_prefix_project(&project_root);
    install_fake_opencode(&bin_dir, &["openai/gpt-5"], None);

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "gpt-5"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["source"].as_str(), Some("alias_prefix"));
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["availability"].as_str(), Some("runnable"));
    assert_eq!(
        stdout["availability_source"].as_str(),
        Some("opencode_probe")
    );
    assert_eq!(
        stdout["runnable_paths"],
        json!([{
            "harness": "opencode",
            "mars_provider": "openai",
            "harness_model_id": "openai/gpt-5"
        }])
    );
}

#[test]
#[serial]
fn resolve_passthrough_uses_loaded_live_probe_for_runnable_availability_without_extra_probe_runs() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = temp.path().join("bin");
    let marker = temp.path().join("opencode-probe-runs.log");
    fs::create_dir_all(&bin_dir).unwrap();
    write_mars_toml(
        &project_root,
        r#"[settings]
harness_order = ["opencode"]
"#,
    );
    write_models_cache(&project_root, &models_cache_json());
    install_fake_opencode(&bin_dir, &["openai/gpt-5"], Some(&marker));

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "openai/gpt-5"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["source"].as_str(), Some("passthrough"));
    assert_eq!(stdout["harness"].as_str(), Some("opencode"));
    assert_eq!(stdout["availability"].as_str(), Some("runnable"));
    assert_eq!(
        stdout["availability_source"].as_str(),
        Some("opencode_probe")
    );
    assert_eq!(
        stdout["runnable_paths"],
        json!([{
            "harness": "opencode",
            "mars_provider": "openai",
            "harness_model_id": "openai/gpt-5"
        }])
    );

    let probe_runs = fs::read_to_string(&marker).expect("probe marker should exist");
    assert_eq!(
        probe_runs.lines().count(),
        1,
        "availability annotation should reuse loaded probe evidence"
    );
}
