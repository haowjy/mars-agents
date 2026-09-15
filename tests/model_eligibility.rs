//! Native auth uncertainty and harness-default launches use the routing assessor.
#[path = "common/mod.rs"]
mod test_common;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn no_model_launch_rejects_logged_out_native_harness() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    #[cfg(windows)]
    let claude = bin.join("claude.bat");
    #[cfg(not(windows))]
    let claude = bin.join("claude");
    let script = std::fs::read_to_string(&claude).unwrap();
    std::fs::write(
        &claude,
        script
            .replace("exit 0", "exit 1")
            .replace("exit /b 0", "exit /b 1"),
    )
    .unwrap();
    std::fs::write(
        root.join("mars.toml"),
        "[settings]\ntargets=[\".claude\",\".codex\"]\nharness_order=[\"claude\",\"codex\"]\n",
    )
    .unwrap();
    let log = root.join("commands.log");
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(["build", "launch-bundle", "--no-refresh-models", "--json"])
        .env("PATH", &bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["claude auth status", "codex login status"]
    );
}

#[test]
fn unknown_native_auth_preserves_an_unverified_model_route() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    #[cfg(windows)]
    std::fs::write(
        bin.join("claude.bat"),
        "@echo off\r\n:waiting\r\ngoto waiting\r\n",
    )
    .unwrap();
    #[cfg(not(windows))]
    std::fs::write(bin.join("claude"), "#!/bin/sh\nexec /bin/sleep 5\n").unwrap();
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"}]}"#,
    )
    .unwrap();
    std::fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".claude\"]\n[models.opus]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n").unwrap();
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(["models", "resolve", "opus", "--no-refresh-models", "--json"])
        .env("PATH", &bin)
        .env("PROBE_LOG", root.join("commands.log"))
        .env("MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS", "1")
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["harness"], "claude", "{value}");
    assert_eq!(value["availability"], "unknown", "{value}");
    let assessment = &value["route_trace"]["assessments"][0];
    assert_eq!(assessment["verdict"], "unverified", "{value}");
    assert_eq!(assessment["reason"], "auth_unknown", "{value}");
    assert!(assessment["skip_reason"].is_null(), "{value}");
}

#[test]
fn live_aliases_share_native_auth_evidence_for_the_invocation() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"}]}"#,
    )
    .unwrap();
    std::fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".claude\"]\n[models.first]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n[models.second]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n").unwrap();
    let log = root.join("commands.log");
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args([
            "models",
            "list",
            "--live",
            "--unavailable",
            "--no-refresh-models",
            "--json",
        ])
        .env("PATH", &bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["aliases"].as_array().unwrap().len(), 2);
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["claude auth status"]
    );
}
