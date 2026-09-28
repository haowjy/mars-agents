//! End-to-end P3 command contracts with isolated catalog and probe caches.
mod common;

use common::*;
use httpmock::MockServer;
use serde_json::{Value, json};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn run(
    root: &std::path::Path,
    env: &std::path::Path,
    api: &str,
    bin: &std::path::Path,
    args: &[&str],
) -> Value {
    let output = mars_cmd(root, env, api)
        .args(args)
        .env("PATH", bin)
        .env("PROBE_LOG", env.join("commands.log"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn list_all_live_aliases_catalog_and_curated_contract() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = install_logging_harnesses(temp.path());
    fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".claude\",\".codex\"]\n[models.fast]\nmodel=\"gpt-5\"\nprovider=\"openai\"\n[models.qualified]\nmodel=\"openai/gpt-5\"\nprovider=\"openai\"\n[models.orphan]\nmodel=\"not-in-catalog\"\nprovider=\"openai\"\n").unwrap();
    write_cache(
        &root,
        vec![
            json!({"id":"gpt-5","provider":"OpenAI","cost_input":1.25,"cost_output":2.5,"description":"General","release_date":"2026-01-01"}),
            json!({"id":"claude-opus-4-6","provider":"anthropic"}),
        ],
        &fresh_fetched_at(),
    );
    fs::write(root.join("mars.curated.toml"), "[[show]]\nharness=\"codex\"\nmodel=\"gpt-5\"\n[[hide]]\nharness=\"claude\"\nmodel=\"claude-opus-4-6\"\n").unwrap();
    let args = ["--json", "models", "list", "--no-refresh-models"];
    let shown = run(&root, temp.path(), &server.url(API_PATH), &bin, &args);
    let rows = shown["models"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{shown}");
    let row = &rows[0];
    assert_eq!(row["harness"], "codex");
    assert_eq!(row["harness_model_id"], "gpt-5");
    assert_eq!(row["provider"], "openai");
    assert_eq!(row["origin"], "both");
    assert_eq!(row["provenance"]["kind"], "inferred");
    assert_eq!(row["via"], "catalog");
    assert_eq!(row["aliases"], json!(["fast", "qualified"]));
    assert_eq!(row["curated"], json!({"decision":"shown","tier":"project"}));
    assert!(row["eligibility"].is_null());
    assert!(row.get("eligibility").is_some());
    assert!(row.get("reason").is_some());
    assert!(shown["diagnostics"].is_array());
    let all = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--live",
            "--no-refresh-models",
        ],
    );
    assert_eq!(all["models"].as_array().unwrap().len(), 2);
    let hidden = all["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["harness"] == "claude")
        .unwrap();
    assert_eq!(
        hidden["curated"],
        json!({"decision":"hidden","tier":"project"})
    );
    assert!(hidden["eligibility"].is_string());
    let narrowed = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--harness",
            "CoDeX",
            "--match",
            "gpt-*",
            "--no-refresh-models",
        ],
    );
    assert_eq!(narrowed["models"].as_array().unwrap().len(), 1);
    let aliases = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "aliases", "--no-refresh-models"],
    );
    assert!(
        aliases["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "fast")
    );
    assert!(
        aliases["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a.get("harness_source").is_none())
    );
    assert!(aliases.get("models").is_none());
    assert!(
        aliases["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "orphan"),
        "aliases may name models absent from Possible without creating list rows"
    );
    let catalog = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "catalog", "--no-refresh-models"],
    );
    assert_eq!(catalog["catalog"].as_array().unwrap().len(), 2);
    let model = catalog["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "gpt-5")
        .unwrap();
    assert_eq!(model["cost_input"], 1.25);
    assert_eq!(model["description"], "General");
    assert_eq!(model["release_date"], "2026-01-01");
    let before = fs::read_to_string(temp.path().join("commands.log")).unwrap_or_default();
    run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "aliases", "--no-refresh-models"],
    );
    run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "catalog", "--no-refresh-models"],
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("commands.log")).unwrap_or_default(),
        before,
        "alias and catalog inventory must not run harness commands"
    );
}

#[test]
fn declared_uninstalled_and_malformed_curated_are_visible() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = temp.path().join("empty-bin");
    fs::create_dir_all(&bin).unwrap();
    write_cache(
        &root,
        vec![json!({"id":"unused","provider":"xai"})],
        &fresh_fetched_at(),
    );
    fs::write(
        root.join("mars.curated.toml"),
        "[[show]]\nharness=\"cursor\"\nmodel=\"composer-2.5\"\n",
    )
    .unwrap();
    let value = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "list", "--live", "--no-refresh-models"],
    );
    let row = &value["models"][0];
    assert_eq!(row["origin"], "declared");
    assert_eq!(row["harness_model_id"], "composer-2.5");
    assert!(row["provenance"].is_null());
    assert_eq!(row["eligibility"], "blocked");
    assert_eq!(row["reason"], "not_installed");
    fs::write(
        root.join("mars.curated.toml"),
        "[[show]]\nharness=\"bogus\"\nmodel=\"x\"\n",
    )
    .unwrap();
    let output = mars_cmd(&root, temp.path(), &server.url(API_PATH))
        .args(["--json", "models", "list", "--no-refresh-models"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mars.curated.toml")
    );
    // Machine views must not read curated files, even malformed ones.
    let aliases = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "aliases", "--no-refresh-models"],
    );
    assert!(aliases["aliases"].is_array());
    let catalog = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "catalog", "--no-refresh-models"],
    );
    assert!(catalog["catalog"].is_array());
    fs::remove_file(root.join("mars.curated.toml")).unwrap();
    let empty = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "list", "--no-refresh-models"],
    );
    assert!(empty["models"].as_array().unwrap().is_empty());
}

#[test]
fn removed_flags_and_visibility_config_report_migration() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    for flag in [
        "--include",
        "--exclude",
        "--providers",
        "--no-visibility",
        "--catalog",
        "--unavailable",
    ] {
        let output = mars_cmd(&root, temp.path(), &server.url(API_PATH))
            .args(["models", "list", flag])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let destination = match flag {
            "--catalog" => "mars models catalog",
            "--no-visibility" => "models list --all",
            "--unavailable" => "models list --all --live",
            _ => "mars.curated.toml",
        };
        assert!(stderr.contains(destination), "{flag}: {stderr}");
    }
    let invalid = mars_cmd(&root, temp.path(), &server.url(API_PATH))
        .args([
            "--json",
            "models",
            "list",
            "--harness",
            "gemini",
            "--no-refresh-models",
        ])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let invalid_json: Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert!(
        invalid_json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("valid harnesses")
    );
    for (name, table) in [
        (
            "mars.toml",
            "[settings.model_visibility]\ninclude=[\"gpt-*\"]\n",
        ),
        (
            "mars.local.toml",
            "[settings.model_visibility]\nproviders=[\"openai\"]\n",
        ),
    ] {
        let path = root.join(name);
        fs::write(&path, table).unwrap();
        let output = mars_cmd(&root, temp.path(), &server.url(API_PATH))
            .args(["--json", "models", "aliases", "--no-refresh-models"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let msg = value["error"]["message"].as_str().unwrap();
        assert!(msg.contains(name), "{msg}");
        if name == "mars.toml" {
            assert!(msg.contains("include = \"gpt-*\" → [[show]]"), "{msg}");
            assert!(!msg.contains("[[hide]]"), "{msg}");
        } else {
            assert!(msg.contains("providers = \"openai\" → [[show]]"), "{msg}");
        }
        fs::write(&path, "[settings]\n").unwrap();
    }
}

#[test]
fn provider_specific_list_rows_keep_independent_provenance() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = install_logging_harnesses(temp.path());
    fs::write(
        root.join("mars.toml"),
        "[settings]\ntargets=[\".opencode\"]\n",
    )
    .unwrap();
    write_cache(
        &root,
        vec![json!({"id":"unused","provider":"xai"})],
        &fresh_fetched_at(),
    );
    let dir = temp.path().join("mars-cache/availability");
    fs::create_dir_all(&dir).unwrap();
    let now = now();
    fs::write(dir.join("opencode-probe.json"),serde_json::to_vec(&json!({
        "schema_version":1,"fetched_at":now-120,"last_attempt_at":now,
        "last_error":"transient failure","result":{"providers":{"openai":true,"xai":true},
        "model_slugs":["openai/gpt-5","xai/gpt-5"],"provider_probe_success":true,"model_probe_success":true,"error":null}
    })).unwrap()).unwrap();
    let value = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--live",
            "--no-refresh-models",
        ],
    );
    let rows = value["models"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{value}");
    assert_ne!(rows[0]["provider"], rows[1]["provider"]);
    for row in rows {
        assert_eq!(row["harness"], "opencode");
        assert_eq!(row["provenance"]["probe"], "opencode");
        assert_eq!(row["provenance"]["kind"], "enumerated");
        assert_eq!(row["provenance"]["latest_attempt_ok"], false);
        assert_eq!(row["provenance"]["last_error"], "transient failure");
        assert!(row["eligibility"].is_string());
        assert!(row["via"].as_str().unwrap().contains("refresh failed"));
    }
    let text = mars_cmd(&root, temp.path(), &server.url(API_PATH))
        .args(["models", "list", "--all", "--no-refresh-models"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.lines().any(|line| line.starts_with("opencode ")),
        "{text}"
    );
    assert!(!text.contains("open_code"), "{text}");
    let curation_column = text.lines().next().unwrap().find("CURATION").unwrap();
    for line in text.lines().filter(|line| line.starts_with("opencode ")) {
        assert!(line[curation_column..].starts_with("shown"), "{text}");
    }
}

#[test]
fn retained_failed_listings_warn_once_per_harness_even_when_hidden() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = install_logging_harnesses(temp.path());
    fs::write(
        root.join("mars.toml"),
        "[settings]\ntargets=[\".pi\",\".cursor\",\".opencode\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("mars.curated.toml"),
        "[[hide]]\nharness=\"*\"\nmodel=\"*\"\n",
    )
    .unwrap();
    write_cache(
        &root,
        vec![json!({"id":"unused","provider":"xai"})],
        &fresh_fetched_at(),
    );
    let dir = temp.path().join("mars-cache/availability");
    fs::create_dir_all(&dir).unwrap();
    let now = now();
    let old = now - 120;
    fs::write(dir.join("pi.json"), serde_json::to_vec(&json!({
        "schema_version":3,"harness":"pi","fetched_at":old,"last_attempt_at":now,
        "last_error":"pi refresh timed out","result":{"binary_path":"pi","version":"1.0","compatible":true,
        "model_probe_success":true,"help_surface_tokens_present":[],"help_surface_tokens_missing":[],
        "model_slugs":["openai/gpt-5","openai-codex/gpt-5"],"error":null}
    })).unwrap()).unwrap();
    fs::write(dir.join("cursor-probe.json"), serde_json::to_vec(&json!({
        "schema_version":1,"fetched_at":old,"last_attempt_at":now,
        "last_error":"cursor auth expired","result":{"slugs":["composer-2.5"],"model_probe_success":true,"error":null}
    })).unwrap()).unwrap();
    fs::write(dir.join("opencode-probe.json"), serde_json::to_vec(&json!({
        "schema_version":1,"fetched_at":old,"last_attempt_at":now,
        "last_error":"opencode refresh timed out","result":{"model_slugs":["openai/gpt-5","xai/grok-4"],
        "model_probe_success":true,"error":null}
    })).unwrap()).unwrap();

    let hidden = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &["--json", "models", "list", "--no-refresh-models"],
    );
    assert!(hidden["models"].as_array().unwrap().is_empty(), "{hidden}");
    let diagnostics = hidden["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 3, "{hidden}");
    for (harness, error) in [
        ("pi", "pi refresh timed out"),
        ("cursor", "cursor auth expired"),
        ("opencode", "opencode refresh timed out"),
    ] {
        let expected =
            format!("{harness}: refresh failed; serving last successful listing: {error}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|value| value.as_str() == Some(&expected))
                .count(),
            1,
            "{hidden}"
        );
    }

    let all = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--live",
            "--no-refresh-models",
        ],
    );
    assert_eq!(all["models"].as_array().unwrap().len(), 5, "{all}");
    assert_eq!(all["diagnostics"], hidden["diagnostics"], "{all}");
    for row in all["models"].as_array().unwrap() {
        assert_eq!(row["provenance"]["latest_attempt_ok"], false, "{all}");
        assert!(row["provenance"]["last_error"].is_string(), "{all}");
        if row["harness"] == "pi" || row["harness"] == "cursor" {
            assert_eq!(row["reason"], "auth_listing_failed", "{all}");
        }
    }
    let output = mars_cmd(&root, temp.path(), &server.url(API_PATH))
        .args(["models", "list", "--no-refresh-models"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for harness in ["pi", "cursor", "opencode"] {
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.contains(&format!(
                    "warning: {harness}: refresh failed; serving last successful listing"
                )))
                .count(),
            1,
            "{stderr}"
        );
    }
    assert!(!stderr.contains("listing unavailable"), "{stderr}");
}

#[test]
fn pi_provider_variants_get_independent_live_verdicts() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = install_logging_harnesses(temp.path());
    fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".pi\"]\n").unwrap();
    write_cache(
        &root,
        vec![json!({"id":"unused","provider":"xai"})],
        &fresh_fetched_at(),
    );
    let dir = temp.path().join("mars-cache/availability");
    fs::create_dir_all(&dir).unwrap();
    let now = now();
    fs::write(dir.join("pi.json"), serde_json::to_vec(&json!({
        "schema_version":3,"harness":"pi","fetched_at":now,"last_attempt_at":now,
        "last_error":null,"result":{"binary_path":"pi","version":"1.0","compatible":true,
        "model_probe_success":true,"help_surface_tokens_present":[],"help_surface_tokens_missing":[],
        "model_slugs":["openai/gpt-5.6-sol","openai-codex/gpt-5.6-sol"],"error":null}
    })).unwrap()).unwrap();
    let value = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--live",
            "--harness",
            "pi",
            "--no-refresh-models",
        ],
    );
    let rows = value["models"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{value}");
    for row in rows {
        assert_eq!(row["eligibility"], "eligible", "{value}");
        assert!(row["reason"].is_null(), "{value}");
    }
    assert_ne!(rows[0]["harness_model_id"], rows[1]["harness_model_id"]);
}

#[test]
fn cold_pi_listing_failure_is_reported_without_possible_rows() {
    let server = MockServer::start();
    let (temp, root) = setup_project(&server);
    let bin = install_logging_harnesses(temp.path());
    fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".pi\"]\n").unwrap();
    write_cache(
        &root,
        vec![json!({"id":"unused","provider":"xai"})],
        &fresh_fetched_at(),
    );
    let dir = temp.path().join("mars-cache/availability");
    fs::create_dir_all(&dir).unwrap();
    let now = now();
    fs::write(
        dir.join("pi.json"),
        serde_json::to_vec(&json!({
            "schema_version":3,"harness":"pi","fetched_at":0,"last_attempt_at":now,
            "last_error":"pi --list-models timed out","result":null
        }))
        .unwrap(),
    )
    .unwrap();
    let value = run(
        &root,
        temp.path(),
        &server.url(API_PATH),
        &bin,
        &[
            "--json",
            "models",
            "list",
            "--all",
            "--harness",
            "pi",
            "--no-refresh-models",
        ],
    );
    assert!(value["models"].as_array().unwrap().is_empty(), "{value}");
    assert!(
        value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message
                .as_str()
                .unwrap()
                .contains("pi: listing unavailable: pi --list-models timed out")),
        "{value}"
    );
}
