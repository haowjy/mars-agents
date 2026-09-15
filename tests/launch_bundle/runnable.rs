//! Final executable-model projection must not undo the selected model or provider.
use serde_json::Value;
use tempfile::tempdir;

use crate::test_common::{install_logging_harnesses, mars_cmd};

fn unverified_bundle(harness: &str, model: Option<&str>, aliases: &str) -> Value {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = install_logging_harnesses(root);
    let log = root.join("commands.log");
    std::fs::write(
        root.join("mars.toml"),
        format!("[settings]\ntargets=[\".{harness}\"]\n{aliases}\n"),
    )
    .unwrap();
    let mut command = mars_cmd(root, root, "http://127.0.0.1:1");
    command.args([
        "build",
        "launch-bundle",
        "--harness",
        harness,
        "--no-refresh-models",
    ]);
    if let Some(model) = model {
        command.args(["--model", model]);
    }
    let output = command
        .env("PATH", bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(std::fs::read_to_string(log).unwrap_or_default().is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn fixed_pi_keeps_named_model_without_probe_evidence() {
    let bundle = unverified_bundle("pi", Some("gpt-5"), "");
    for field in ["model", "model_token", "harness_model"] {
        assert_eq!(bundle["routing"][field], "gpt-5", "{field}: {bundle}");
    }
    assert_eq!(bundle["routing"]["harness"], "pi");
    assert_eq!(bundle["routing"]["harness_model_confidence"], "unknown");
    assert!(!bundle["warnings"].to_string().contains("clearing model"));
}

#[test]
fn unresolved_alias_retains_provider_in_executable_model() {
    for harness in ["opencode", "pi"] {
        let bundle = unverified_bundle(
            harness,
            Some("pending"),
            "[models.pending]\nprovider=\"openrouter\"\nmatch=[\"not-in-catalog-*\"]",
        );
        assert_eq!(bundle["routing"]["model"], "pending", "{bundle}");
        assert_eq!(bundle["routing"]["model_token"], "pending", "{bundle}");
        assert_eq!(
            bundle["routing"]["harness_model"], "openrouter/pending",
            "{bundle}"
        );
    }
}

#[test]
fn fixed_pi_without_requested_model_keeps_native_default() {
    let bundle = unverified_bundle("pi", None, "");
    for field in ["model", "model_token", "harness_model"] {
        assert_eq!(bundle["routing"][field], "", "{field}: {bundle}");
    }
}
