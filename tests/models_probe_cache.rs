mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use common::{fresh_fetched_at, now_unix_secs};

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

fn probe_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("availability").join("opencode-probe.json")
}

fn models_cache_json() -> String {
    serde_json::to_string_pretty(&json!({
        "models": [{
            "id": "openai/gpt-5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        }],
        "fetched_at": fresh_fetched_at()
    }))
    .unwrap()
}

fn probe_cache_json(fetched_at: u64) -> String {
    serde_json::to_string_pretty(&json!({
        "schema_version": 1,
        "fetched_at": fetched_at,
        "last_attempt_at": fetched_at,
        "last_error": null,
        "result": {
            "providers": {"openai": true},
            "model_slugs": ["openai/gpt-5"],
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }
    }))
    .unwrap()
}

fn setup_project(project_root: &Path) {
    write_mars_toml(
        project_root,
        r#"[settings]

[models.fast]
harness = "opencode"
model = "openai/gpt-5"
"#,
    );
    write_models_cache(project_root, &models_cache_json());
}

fn install_fake_opencode(temp: &TempDir) -> PathBuf {
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    #[cfg(windows)]
    {
        let path = bin_dir.join("opencode.bat");
        fs::write(
            &path,
            "@echo off\r\nif \"%1\"==\"providers\" (echo *  OpenAI oauth) else (echo openai/gpt-5)\r\n",
        )
        .unwrap();
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = bin_dir.join("opencode");
        fs::write(
            &path,
            "#!/bin/sh\nif [ \"$1\" = \"providers\" ]; then echo '*  OpenAI oauth'; else echo 'openai/gpt-5'; fi\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    bin_dir
}

fn prepend_path(bin_dir: &Path) -> String {
    let current = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf()).chain(std::env::split_paths(&current)),
    )
    .unwrap()
    .to_string_lossy()
    .into_owned()
}

#[test]
#[serial]
fn resolve_reads_prepopulated_probe_cache_hit() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = install_fake_opencode(&temp);
    setup_project(&project_root);
    write_probe_cache(&cache_dir, &probe_cache_json(now_unix_secs()));

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .env_remove("MARS_OFFLINE")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"probe_cache\": \"hit\""))
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["resolved_model"].as_str(), Some("openai/gpt-5"));
    assert_eq!(stdout["probe_cache"].as_str(), Some("hit"));
}

#[test]
#[serial]
fn no_refresh_models_skips_probe_refresh_even_with_stale_cache() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    setup_project(&project_root);
    write_probe_cache(&cache_dir, &probe_cache_json(1));

    let output = mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast", "--no-refresh-models"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("MARS_OFFLINE", "1")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["probe_cache"].as_str(), Some("skipped"));
}

#[test]
#[serial]
fn cold_probe_write_creates_valid_json_cache_file() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("mars-cache");
    let bin_dir = install_fake_opencode(&temp);
    setup_project(&project_root);

    mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .env_remove("MARS_OFFLINE")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"probe_cache\": \"miss\""));

    let raw = fs::read_to_string(probe_cache_path(&cache_dir)).unwrap();
    let cache: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(cache["schema_version"].as_u64(), Some(1));
    assert!(cache["fetched_at"].as_u64().is_some());
    assert!(cache["last_attempt_at"].as_u64().is_some());
    assert_eq!(cache["last_error"], Value::Null);
    assert_eq!(
        cache["result"]["provider_probe_success"].as_bool(),
        Some(true)
    );
}

#[test]
#[serial]
fn probe_cache_file_uses_mars_cache_dir_availability_location() {
    let temp = TempDir::new().unwrap();
    let project_root = temp.path().join("project");
    let cache_dir = temp.path().join("custom-cache-root");
    let bin_dir = install_fake_opencode(&temp);
    setup_project(&project_root);

    mars_cmd()
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "resolve", "fast"])
        .env("MARS_CACHE_DIR", &cache_dir)
        .env("PATH", prepend_path(&bin_dir))
        .env_remove("MARS_OFFLINE")
        .assert()
        .success();

    assert!(
        probe_cache_path(&cache_dir).exists(),
        "expected probe cache at MARS_CACHE_DIR/availability/opencode-probe.json"
    );
}
