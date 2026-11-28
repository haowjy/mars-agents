#![allow(dead_code)]

use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

pub const API_PATH: &str = "/api.json";

pub fn create_source(
    dir: &TempDir,
    name: &str,
    agents: &[(&str, &str)],
    skills: &[(&str, &str)],
) -> std::path::PathBuf {
    let source_dir = dir.child(name);
    source_dir.create_dir_all().unwrap();

    if !agents.is_empty() {
        let agents_dir = source_dir.child("agents");
        agents_dir.create_dir_all().unwrap();
        for (agent_name, content) in agents {
            agents_dir
                .child(format!("{agent_name}.md"))
                .write_str(content)
                .unwrap();
        }
    }

    if !skills.is_empty() {
        let skills_dir = source_dir.child("skills");
        skills_dir.create_dir_all().unwrap();
        for (skill_name, content) in skills {
            let skill_sub = skills_dir.child(skill_name);
            skill_sub.create_dir_all().unwrap();
            skill_sub.child("SKILL.md").write_str(content).unwrap();
        }
    }

    source_dir.to_path_buf()
}

pub fn create_mcp_source(dir: &TempDir, name: &str, server_name: &str) -> std::path::PathBuf {
    let source_dir = dir.child(name);
    let mcp_dir = source_dir.child("mcp").child(server_name);
    mcp_dir.create_dir_all().unwrap();
    mcp_dir
        .child("mcp.toml")
        .write_str(
            r#"
command = "npx"
args = ["-y", "example@latest"]
visibility = "exported"
"#,
        )
        .unwrap();
    source_dir.to_path_buf()
}

/// The mars binary with no user `config.toml`, no user level and its own cache root: the
/// developer's own settings and items must not reach a fixture. Helpers that test the user layer
/// set `MARS_HOME` again.
pub fn mars() -> Command {
    let mut cmd = Command::cargo_bin("mars").unwrap();
    cmd.env("MARS_HOME", no_user_home())
        .env_remove("MARS_CONFIG_DIR")
        .env("MARS_CACHE_DIR", fresh_cache_dir())
        .env(
            "MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS",
            FIXTURE_AUTH_TIMEOUT_SECS,
        );
    cmd
}

/// A fake harness answers its auth probe in milliseconds. Mars' own 2 s default would turn a
/// loaded runner's slow spawn into an `unknown` auth state; a test of the timeout sets its own.
const FIXTURE_AUTH_TIMEOUT_SECS: &str = "30";

/// `mars()` with the cache root of a project whose test root is its parent directory: the one
/// [`write_cache`] fills and [`configure_assert_cmd`] gives its command.
pub fn mars_for(project_root: impl AsRef<Path>) -> Command {
    let mut cmd = mars();
    cmd.env("MARS_CACHE_DIR", cache_dir(project_root.as_ref()));
    cmd
}

/// A cache root no other command sees: the models snapshot is per user, so a fixture that shared
/// the developer's, or another test's, would read its catalog. The directories are removed when
/// the test process exits.
fn fresh_cache_dir() -> PathBuf {
    use std::sync::{Mutex, Once, PoisonError, atomic::AtomicU32, atomic::Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    static DIRS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    static REMOVE_AT_EXIT: Once = Once::new();
    extern "C" fn remove_all() {
        for dir in DIRS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
        {
            let _ = fs::remove_dir_all(dir);
        }
    }
    REMOVE_AT_EXIT.call_once(|| {
        // SAFETY: `remove_all` is a plain function that touches only the statics above.
        unsafe { libc::atexit(remove_all) };
    });
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "test-cache-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    DIRS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(dir.clone());
    dir
}

/// A user folder that is never created.
pub fn no_user_home() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-user-home")
}

/// Offline CLI fixture; all writable environment state stays under this test root.
/// `.mars` keeps fixture state out of declared-package source discovery/staging.
pub fn offline_mars(root: &Path) -> Command {
    let mut cmd = mars();
    configure_assert_cmd(
        &mut cmd,
        &root.join(".mars/test-env"),
        "http://127.0.0.1:9/api.json",
    );
    cmd.env("MARS_OFFLINE", "1").env("PATH", "");
    cmd
}

/// Render a path with separators that are safe in TOML/JSON string fixtures
/// and accepted by Windows as well as POSIX.
pub fn portable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn setup_synced_project(
    dir: &TempDir,
    project_name: &str,
    source_name: &str,
    agents: &[(&str, &str)],
    skills: &[(&str, &str)],
) -> std::path::PathBuf {
    let source = create_source(dir, source_name, agents, skills);
    let project = dir.child(project_name);
    project.create_dir_all().unwrap();

    let toml = format!(
        "[dependencies]\n{source_name} = {{ path = \"{}\" }}\n",
        portable_path(&source)
    );
    project.child("mars.toml").write_str(&toml).unwrap();

    offline_mars(dir.path())
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    project.to_path_buf()
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn fresh_fetched_at() -> String {
    now_unix_secs().saturating_sub(60).to_string()
}

pub fn stale_fetched_at() -> String {
    now_unix_secs().saturating_sub(25 * 3600).to_string()
}

pub fn sample_catalog_json() -> Value {
    json!({
        "anthropic": {
            "models": {
                "claude-opus-4-6": {
                    "id": "claude-opus-4-6",
                    "name": "Claude Opus 4.6",
                    "release_date": "2026-02-05",
                    "limit": {
                        "context": 1000000,
                        "output": 128000
                    }
                }
            }
        },
        "openai": {
            "models": {
                "gpt-5": {
                    "id": "gpt-5",
                    "name": "GPT-5",
                    "release_date": "2025-06-01",
                    "limit": {
                        "context": 400000,
                        "output": 128000
                    }
                }
            }
        }
    })
}

pub fn sample_cached_models() -> Vec<Value> {
    vec![
        json!({
            "id": "claude-opus-4-6",
            "provider": "Anthropic",
            "release_date": "2026-02-05"
        }),
        json!({
            "id": "gpt-5",
            "provider": "OpenAI",
            "release_date": "2025-06-01"
        }),
    ]
}

/// The shared models cache directory of a project whose test root is its parent directory.
pub fn cache_dir(project_root: &Path) -> PathBuf {
    project_root
        .parent()
        .expect("project root has a test root")
        .join("mars-cache")
}

/// The shared models snapshot of a project whose test root is its parent directory, as
/// `configure_assert_cmd` lays out `MARS_CACHE_DIR`.
pub fn cache_path(project_root: &Path) -> PathBuf {
    cache_dir(project_root).join("models-catalog-v2.json")
}

pub fn write_local_source_with_model_alias(
    temp_root: &Path,
    source_dir_name: &str,
    alias_name: &str,
    model_id: &str,
) -> PathBuf {
    let source_root = temp_root.join(source_dir_name);
    let agents_dir = source_root.join("agents");
    fs::create_dir_all(&agents_dir).expect("failed to create local source agents dir");
    fs::write(
        agents_dir.join("fixture.md"),
        "---\n---\n# Fixture agent for models-cache tests\n",
    )
    .expect("failed to write local source fixture agent");

    let source_manifest = format!(
        r#"[package]
schema = 1
name = "{source_dir_name}"
version = "0.1.0"

[models."{alias_name}"]
harness = "codex"
model = "{model_id}"
description = "fixture alias for models cache tests"
"#
    );
    fs::write(source_root.join("mars.toml"), source_manifest)
        .expect("failed to write local source mars.toml");
    source_root
}

pub fn model_ids_from_catalog_json(stdout: &[u8]) -> BTreeSet<String> {
    let payload: Value =
        serde_json::from_slice(stdout).expect("models catalog --json must be valid JSON");
    payload["catalog"]
        .as_array()
        .expect("models catalog JSON should include catalog array")
        .iter()
        .filter_map(|model| model["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

/// Writes the shared snapshot: `models` are flat rows, grouped here by provider key the way
/// the snapshot stores them (a row without `provider_key` is keyed by its lowercased display name).
pub fn write_cache(project_root: &Path, models: Vec<Value>, fetched_at: &str) {
    let path = cache_path(project_root);
    fs::create_dir_all(path.parent().unwrap()).expect("failed to create cache directory");
    let mut providers: Vec<(String, Vec<Value>)> = Vec::new();
    for mut model in models {
        let key = model["provider_key"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                model["provider"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            });
        model["provider_key"] = json!(key);
        match providers.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, rows)) => rows.push(model),
            None => providers.push((key, vec![model])),
        }
    }
    let cache = json!({
        "providers": providers
            .into_iter()
            .map(|(key, models)| json!({ "key": key, "models": models }))
            .collect::<Vec<_>>(),
        "fetched_at": fetched_at,
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&cache).expect("failed to serialize cache fixture"),
    )
    .expect("failed to write cache fixture");
}

/// How many model rows a snapshot read by [`read_cache_json`] holds, across its providers.
pub fn cached_model_count(cache: &Value) -> usize {
    cache["providers"]
        .as_array()
        .expect("snapshot.providers should be an array")
        .iter()
        .map(|provider| provider["models"].as_array().map_or(0, Vec::len))
        .sum()
}

pub fn read_cache_json(project_root: &Path) -> Value {
    let raw = fs::read_to_string(cache_path(project_root)).expect("failed to read cache file");
    serde_json::from_str(&raw).expect("failed to parse cache file JSON")
}

pub fn read_cache_raw(project_root: &Path) -> String {
    fs::read_to_string(cache_path(project_root)).expect("failed to read cache file")
}

pub fn configure_assert_cmd(cmd: &mut Command, temp_root: &Path, api_url: &str) {
    let home = temp_root.join("home");
    let xdg_config = temp_root.join("xdg-config");
    let xdg_data = temp_root.join("xdg-data");
    let mars_home = temp_root.join("mars-home");
    let mars_cache = temp_root.join("mars-cache");

    for dir in [&home, &xdg_config, &xdg_data, &mars_home, &mars_cache] {
        fs::create_dir_all(dir).expect("failed to create isolated env directory");
    }

    cmd.env("MARS_MODELS_API_URL", api_url)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", &xdg_config)
        .env("LOCALAPPDATA", &xdg_data)
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_DATA_HOME", &xdg_data)
        .env("MARS_HOME", &mars_home)
        .env_remove("MARS_CONFIG_DIR")
        .env("MARS_CACHE_DIR", &mars_cache)
        .env("NO_COLOR", "1")
        .env_remove("MARS_OFFLINE")
        .env_remove("MERIDIAN_MANAGED");
}

pub fn configure_std_cmd(cmd: &mut StdCommand, temp_root: &Path, api_url: &str) {
    let home = temp_root.join("home");
    let xdg_config = temp_root.join("xdg-config");
    let xdg_data = temp_root.join("xdg-data");
    let mars_home = temp_root.join("mars-home");
    let mars_cache = temp_root.join("mars-cache");

    for dir in [&home, &xdg_config, &xdg_data, &mars_home, &mars_cache] {
        fs::create_dir_all(dir).expect("failed to create isolated env directory");
    }

    cmd.env(
        "MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS",
        FIXTURE_AUTH_TIMEOUT_SECS,
    )
    .env("MARS_MODELS_API_URL", api_url)
    .env("HOME", &home)
    .env("USERPROFILE", &home)
    .env("APPDATA", &xdg_config)
    .env("LOCALAPPDATA", &xdg_data)
    .env("XDG_CONFIG_HOME", &xdg_config)
    .env("XDG_DATA_HOME", &xdg_data)
    .env("MARS_HOME", &mars_home)
    .env_remove("MARS_CONFIG_DIR")
    .env("MARS_CACHE_DIR", &mars_cache)
    .env("NO_COLOR", "1")
    .env_remove("MARS_OFFLINE")
    .env_remove("MERIDIAN_MANAGED");
}

pub fn mars_cmd(project_root: &Path, temp_root: &Path, api_url: &str) -> Command {
    let mut cmd = Command::cargo_bin("mars").expect("failed to locate mars test binary");
    configure_assert_cmd(&mut cmd, temp_root, api_url);
    cmd.arg("--root").arg(project_root);
    cmd
}

pub fn init_project(project_root: &Path, temp_root: &Path, api_url: &str) {
    fs::create_dir_all(project_root).expect("failed to create project root");

    let mut cmd = mars_cmd(project_root, temp_root, api_url);
    cmd.arg("init");
    cmd.assert().success();
}

pub fn setup_project(server: &MockServer) -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().expect("failed to create temp dir");
    let project_root = temp.path().join("project");
    init_project(&project_root, temp.path(), &server.url(API_PATH));
    (temp, project_root)
}

/// Log every fake harness invocation without executing a native tool.
pub fn install_logging_harnesses(root: &Path) -> PathBuf {
    let bin = root.join("logging-harness-bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["claude", "codex", "opencode", "pi", "agy"] {
        #[cfg(windows)]
        fs::write(
            bin.join(format!("{name}.bat")),
            format!("@echo off\r\necho {name} %*>>\"%PROBE_LOG%\"\r\nexit /b 0\r\n"),
        )
        .unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin.join(name);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s\\n' \"{name} $*\" >> \"$PROBE_LOG\"\nexit 0\n"),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    bin
}
