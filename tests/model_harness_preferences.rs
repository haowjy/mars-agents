//! Standalone alias routing uses the same preference and permission rules as launches.
#[path = "common/mod.rs"]
mod test_common;

use serde_json::Value;
use tempfile::tempdir;

fn model_command(
    settings: &str,
    aliases: &str,
    args: &[&str],
    reject_codex_auth: bool,
) -> (Value, i32, Vec<String>) {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    if reject_codex_auth {
        #[cfg(windows)]
        let codex = bin.join("codex.bat");
        #[cfg(not(windows))]
        let codex = bin.join("codex");
        let script = std::fs::read_to_string(&codex).unwrap();
        std::fs::write(
            codex,
            script
                .replace("exit 0", "exit 1")
                .replace("exit /b 0", "exit /b 1"),
        )
        .unwrap();
    }
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join("mars.toml"),
        format!("[settings]\n{settings}\n{aliases}\n"),
    )
    .unwrap();
    std::fs::write(root.join(".mars/models-cache.json"), r#"{"fetched_at":null,"models":[{"id":"gpt-5","provider":"openai"},{"id":"gpt-5","provider":"openrouter"}]}"#).unwrap();
    let log = root.join("commands.log");
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(args)
        .args(["--json", "--no-refresh-models"])
        .env("PATH", &bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let calls = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    (value, output.status.code().unwrap(), calls)
}

#[test]
fn exact_and_live_aliases_skip_excluded_or_unverified_preferences() {
    for preference in ["claude", "opencode"] {
        for args in [
            vec!["models", "resolve", "fast"],
            vec!["models", "list", "--live"],
        ] {
            let (value, status, calls) = model_command(
                "targets=[\".codex\",\".opencode\"]\nharness_order=[\"opencode\",\"codex\"]",
                &format!(
                    "[models.fast]\nmodel=\"gpt-5\"\nprovider=\"openai\"\nharness=\"{preference}\""
                ),
                &args,
                false,
            );
            assert_eq!(status, 0, "{args:?}: {value}");
            let entry = value
                .get("aliases")
                .map(|aliases| &aliases[0])
                .unwrap_or(&value);
            assert_eq!(entry["model_id"], "gpt-5", "{value}");
            assert_eq!(entry["harness"], "codex", "{value}");
            assert_eq!(entry["harness_source"], "auto_detected", "{value}");
            assert_eq!(entry["availability"], "runnable", "{value}");
            assert_eq!(calls, ["codex login status"], "{value}");
        }
    }
}

#[test]
fn rejected_preference_can_yield_to_an_unverified_route_without_changing_model() {
    for args in [
        vec!["models", "resolve", "fast"],
        vec!["models", "list", "--live"],
    ] {
        let (value, status, calls) = model_command(
            "targets=[\".codex\",\".opencode\"]",
            "[models.fast]\nmodel=\"gpt-5\"\nprovider=\"openai\"\nharness=\"codex\"",
            &args,
            true,
        );
        assert_eq!(status, 0, "{value}");
        let entry = value
            .get("aliases")
            .map(|aliases| &aliases[0])
            .unwrap_or(&value);
        assert_eq!(entry["model_id"], "gpt-5", "{value}");
        assert_eq!(entry["harness"], "opencode", "{value}");
        assert_eq!(entry["harness_source"], "auto_detected", "{value}");
        assert_eq!(entry["availability"], "unknown", "{value}");
        assert_eq!(entry["runnable_paths"], serde_json::json!([]), "{value}");
        assert_eq!(calls, ["codex login status"], "{value}");
    }
}

#[test]
fn alias_prefix_retains_base_harness_preference() {
    let (value, status, calls) = model_command(
        "targets=[\".pi\",\".opencode\"]\nharness_order=[\"opencode\",\"pi\"]",
        "[models.gpt]\nmodel=\"gpt-5\"\nprovider=\"openai\"\nharness=\"pi\"",
        &["models", "resolve", "gpt-5"],
        false,
    );
    assert_eq!(status, 0, "{value}");
    assert_eq!(value["model_id"], "gpt-5", "{value}");
    assert_eq!(value["harness"], "pi", "{value}");
    assert_eq!(
        value["route_trace"]["model_attempts"][0]["source"], "alias",
        "{value}"
    );
    assert_eq!(
        value["route_trace"]["model_attempts"][0]["selection_kind"], "auto",
        "{value}"
    );
    assert_eq!(value["availability"], "unknown", "{value}");
    assert!(calls.is_empty(), "{calls:?}");
}

#[test]
fn alias_prefix_preserves_provider_constraint_before_native_auth() {
    let (value, status, calls) = model_command(
        "targets=[\".codex\"]",
        "[models.gpt]\nmodel=\"gpt-5\"\nprovider=\"openrouter\"\nharness=\"codex\"",
        &["models", "resolve", "gpt-5"],
        false,
    );
    assert_eq!(status, 1, "{value}");
    assert_eq!(value["model_id"], "gpt-5", "{value}");
    assert!(value["harness"].is_null(), "{value}");
    assert_eq!(value["availability"], "unavailable", "{value}");
    assert_eq!(
        value["route_trace"]["model_attempts"][0]["assessments"][0]["reason"],
        "provider_constraint_unsatisfied",
        "{value}"
    );
    assert!(calls.is_empty(), "{calls:?}");
}
