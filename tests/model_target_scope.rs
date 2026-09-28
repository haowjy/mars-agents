//! Standalone inventory and resolution honor configured harness targets.
#[path = "common/mod.rs"]
mod test_common;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn curated_live_inventory_does_not_probe_out_of_scope_harnesses() {
    for (targets, expected_rows) in [
        ("targets = []", 0),
        ("targets = [\".agents\"]", 0),
        ("targets = [\".codex\"]", 1),
        ("", 2),
    ] {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let bin = test_common::install_logging_harnesses(root);
        let log = root.join("commands.log");
        std::fs::create_dir(root.join(".mars")).unwrap();
        std::fs::write(root.join("mars.toml"), format!("[settings]\n{targets}\n")).unwrap();
        std::fs::write(root.join(".mars/models-cache.json"),
            r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"},{"id":"gpt-5","provider":"openai"}]}"#).unwrap();
        let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
            .args(["models", "list", "--live", "--no-refresh-models", "--json"])
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
        assert_eq!(
            value["models"].as_array().unwrap().len(),
            expected_rows,
            "{value}"
        );
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        assert_eq!(
            calls.contains("claude auth status"),
            targets.is_empty(),
            "{targets}: {calls}"
        );
    }
}

#[test]
fn live_inventory_marks_logged_out_native_row_blocked() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    #[cfg(windows)]
    let claude = bin.join("claude.bat");
    #[cfg(not(windows))]
    let claude = bin.join("claude");
    let script = std::fs::read_to_string(&claude).unwrap();
    #[cfg(windows)]
    let logged_out_script = script.replace("exit /b 0", "exit /b 1");
    #[cfg(not(windows))]
    let logged_out_script = script.replace("exit 0", "exit 1");
    std::fs::write(&claude, logged_out_script).unwrap();
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join("mars.toml"),
        "[settings]\ntargets=[\".claude\",\".codex\"]\n",
    )
    .unwrap();
    std::fs::write(root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"},{"id":"gpt-5","provider":"openai"}]}"#).unwrap();
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(["models", "list", "--live", "--no-refresh-models", "--json"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = value["models"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|row| row["harness"] == "claude").unwrap()["eligibility"],
        "blocked"
    );
    assert_eq!(
        rows.iter().find(|row| row["harness"] == "codex").unwrap()["eligibility"],
        "eligible"
    );
}
