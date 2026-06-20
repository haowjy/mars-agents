mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use common::*;

const EXPLORER_AGENT: &str = "---
name: explorer
description: Explores code
model: gpt55
---
# Explorer
";

fn setup_model_prompting_project(dir: &TempDir) -> std::path::PathBuf {
    let source = create_source(dir, "src", &[("explorer", EXPLORER_AGENT)], &[]);
    let project = dir.child("proj");
    project.create_dir_all().unwrap();

    let toml = format!(
        r#"[dependencies]
src = {{ path = "{}" }}

[models.gpt55]
harness = "codex"
model = "gpt-5"
prompting = "Brief GPT with tight acceptance criteria."

[models.naked]
harness = "codex"
model = "gpt-5"

[models.explorer]
harness = "codex"
model = "gpt-5"
prompting = "This model alias should lose to the agent ref."
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    write_cache(project.path(), sample_cached_models(), &fresh_fetched_at());

    project.to_path_buf()
}

fn install_fake_harnesses(temp_root: &Path, harnesses: &[&str]) -> PathBuf {
    let bin_dir = temp_root.join("harness-bin");
    fs::create_dir_all(&bin_dir).unwrap();

    for harness in harnesses {
        #[cfg(windows)]
        {
            let script = if *harness == "opencode" {
                "@echo off\r\nif \"%~1\"==\"models\" (\r\n  echo openai/gpt-5\r\n  exit /b 0\r\n)\r\nexit /b 0\r\n"
            } else {
                "@echo off\r\nexit /b 0\r\n"
            };
            fs::write(bin_dir.join(format!("{harness}.bat")), script).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = bin_dir.join(harness);
            let script = if *harness == "opencode" {
                "#!/bin/sh\nif [ \"$1\" = \"models\" ]; then\n  printf '%s\\n' 'openai/gpt-5'\n  exit 0\nfi\nexit 0\n"
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

fn replace_path_with(bin_dir: &Path) -> String {
    bin_dir.to_string_lossy().into_owned()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_opencode_probe_cache(cache_root: &Path, model_slugs: &[&str]) {
    let cache_path = cache_root.join("availability").join("opencode-probe.json");
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let now = now_unix_secs();
    let payload = json!({
        "schema_version": 1,
        "fetched_at": now,
        "last_attempt_at": now,
        "last_error": null,
        "result": {
            "providers": { "openai": true },
            "model_slugs": model_slugs,
            "provider_probe_success": true,
            "model_probe_success": true,
            "error": null
        }
    });
    fs::write(cache_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

#[test]
fn models_prompting_json_resolves_agent_first_and_uses_agent_model_guidance() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);
    let bin_dir = install_fake_harnesses(dir.path(), &["codex"]);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "explorer",
            "--root",
            project.to_str().unwrap(),
        ])
        .env("PATH", replace_path_with(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref"], "explorer");
    assert_eq!(json["ref_kind"], "agent");
    assert_eq!(json["agent_name"], "explorer");
    assert_eq!(json["model_alias"], "gpt55");
    assert_eq!(json["model_name"], "gpt-5");
    assert_eq!(json["found"], true);
    assert_eq!(
        json["prompting"],
        "Brief GPT with tight acceptance criteria."
    );
}

#[test]
fn models_prompting_at_agent_ref_resolves_agent() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);
    let bin_dir = install_fake_harnesses(dir.path(), &["codex"]);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "@explorer",
            "--root",
            project.to_str().unwrap(),
        ])
        .env("PATH", replace_path_with(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref"], "@explorer");
    assert_eq!(json["ref_kind"], "agent");
    assert_eq!(json["agent_name"], "explorer");
    assert_eq!(json["model_alias"], "gpt55");
    assert_eq!(json["model_name"], "gpt-5");
    assert_eq!(
        json["prompting"],
        "Brief GPT with tight acceptance criteria."
    );
}

#[test]
fn models_prompting_json_resolves_direct_model_alias() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "gpt55",
            "--root",
            project.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref"], "gpt55");
    assert_eq!(json["ref_kind"], "model");
    assert_eq!(json["agent_name"], Value::Null);
    assert_eq!(json["model_alias"], "gpt55");
    assert_eq!(json["model_name"], "gpt-5");
    assert_eq!(json["found"], true);
    assert_eq!(
        json["prompting"],
        "Brief GPT with tight acceptance criteria."
    );
}

#[test]
fn models_prompt_singular_subcommand_stays_rejected() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);

    mars()
        .args([
            "models",
            "prompt",
            "gpt55",
            "--root",
            project.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand 'prompt'"));
}

#[test]
fn models_prompting_direct_auto_alias_refreshes_catalog_for_model_name() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("fixture", "# Fixture\n")], &[]);
    let project = dir.child("proj");
    project.create_dir_all().unwrap();
    let toml = format!(
        r#"[dependencies]
src = {{ path = "{}" }}

[models.latestgpt]
match = ["gpt-5*"]
provider = "OpenAI"
prompting = "Use refreshed GPT guidance."
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();
    mars()
        .args([
            "sync",
            "--no-refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "latestgpt",
            "--refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MARS_MODELS_API_URL", server.url(API_PATH))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref_kind"], "model");
    assert_eq!(json["model_alias"], "latestgpt");
    assert_eq!(json["model_name"], "gpt-5");
    assert_eq!(json["prompting"], "Use refreshed GPT guidance.");
}

#[test]
fn models_prompting_agent_uses_model_policy_fallback_result() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "src",
        &[(
            "reviewer",
            r#"---
name: reviewer
model: gpt55
model-policies:
  - match:
      alias: gpt55
  - match:
      alias: sonnet
---
# Reviewer
"#,
        )],
        &[],
    );
    let project = dir.child("proj");
    project.create_dir_all().unwrap();
    let toml = format!(
        r#"[dependencies]
src = {{ path = "{}" }}

[settings]
targets = [".claude"]

[models.gpt55]
model = "gpt-5"
prompting = "GPT guidance should not be used after fallback."

[models.sonnet]
model = "claude-opus-4-6"
prompting = "Use Claude-specific review guidance."
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    write_cache(project.path(), sample_cached_models(), &fresh_fetched_at());
    let bin_dir = install_fake_harnesses(dir.path(), &["claude"]);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "reviewer",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("PATH", replace_path_with(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref_kind"], "agent");
    assert_eq!(json["model_alias"], "sonnet");
    assert_eq!(json["model_name"], "claude-opus-4-6");
    assert_eq!(json["prompting"], "Use Claude-specific review guidance.");
}

#[test]
fn models_prompting_agent_does_not_use_pre_routing_token_after_model_clear() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);
    let local = r#"[agents.explorer]
harness = "opencode"
"#;
    fs::write(project.join("mars.local.toml"), local).unwrap();
    let bin_dir = install_fake_harnesses(dir.path(), &["opencode"]);
    let cache_root = dir.path().join("mars-cache");
    write_opencode_probe_cache(&cache_root, &["openai/not-gpt-5"]);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "explorer",
            "--root",
            project.to_str().unwrap(),
        ])
        .env("PATH", replace_path_with(&bin_dir))
        .env("MARS_CACHE_DIR", &cache_root)
        .env("MARS_PROBE_CACHE_TTL_SECS", "60")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref_kind"], "agent");
    assert_eq!(json["model_alias"], Value::Null);
    assert_eq!(json["model_name"], Value::Null);
    assert_eq!(json["prompting"], Value::Null);
}

#[test]
fn models_prompting_file_stem_agent_match_beats_profile_name_collision() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "src",
        &[
            (
                "aaa",
                r#"---
name: explorer
model: sonnet
---
# Name collision
"#,
            ),
            (
                "explorer",
                r#"---
name: stemmed-explorer
model: gpt55
---
# Stem match
"#,
            ),
        ],
        &[],
    );
    let project = dir.child("proj");
    project.create_dir_all().unwrap();
    let toml = format!(
        r#"[dependencies]
src = {{ path = "{}" }}

[models.gpt55]
harness = "codex"
model = "gpt-5"
prompting = "Use the stem-matched agent model."

[models.sonnet]
harness = "claude"
model = "claude-opus-4-6"
prompting = "Wrong profile-name collision guidance."
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    write_cache(project.path(), sample_cached_models(), &fresh_fetched_at());
    let bin_dir = install_fake_harnesses(dir.path(), &["codex", "claude"]);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "explorer",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("PATH", replace_path_with(&bin_dir))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref_kind"], "agent");
    assert_eq!(json["agent_name"], "stemmed-explorer");
    assert_eq!(json["model_alias"], "gpt55");
    assert_eq!(json["prompting"], "Use the stem-matched agent model.");
}

#[test]
fn models_prompting_known_model_without_guidance_exits_zero_and_shows_examples() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);

    mars()
        .args([
            "models",
            "prompting",
            "naked",
            "--root",
            project.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "No prompting guidance defined for model alias `naked`.",
        ))
        .stdout(predicate::str::contains("mars models prompting @explorer"))
        .stdout(predicate::str::contains("mars models prompting gpt55"));
}

#[test]
fn models_prompting_unknown_ref_json_exits_nonzero_with_found_false() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "missing",
            "--root",
            project.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|_| panic!("models prompting --json must be valid JSON:\n{stdout}"));

    assert_eq!(json["ref"], "missing");
    assert_eq!(json["ref_kind"], Value::Null);
    assert_eq!(json["found"], false);
    assert_eq!(json["prompting"], Value::Null);
}
