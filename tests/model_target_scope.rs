//! Standalone identity resolution must not probe or advertise excluded routes.
#[path = "common/mod.rs"]
mod test_common;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn standalone_model_commands_respect_target_permission() {
    for targets in [
        "targets = []",
        "targets = [\".agents\", \"output/agents\"]",
        "targets = [\".codex\"]",
        "",
    ] {
        for command in [
            vec!["models", "resolve", "opus"],
            vec!["models", "resolve", "opus-4-6"],
            vec!["models", "list", "--live", "--unavailable"],
        ] {
            for refresh in ["--no-refresh-models", "--refresh-models"] {
                let server = httpmock::MockServer::start();
                server.mock(|when, then| {
                    when.path(test_common::API_PATH);
                    then.json_body(test_common::sample_catalog_json());
                });
                let temp = tempdir().unwrap();
                let root = temp.path();
                let bin = test_common::install_logging_harnesses(root);
                let log = root.join("commands.log");
                std::fs::create_dir(root.join(".mars")).unwrap();
                std::fs::write(root.join("mars.toml"), format!("[settings]\n{targets}\n[models.opus]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n")).unwrap();
                std::fs::write(
                root.join(".mars/models-cache.json"),
                r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"}]}"#,
            )
            .unwrap();
                let mut cmd = test_common::mars_cmd(root, root, &server.url(test_common::API_PATH));
                let output = cmd
                    .args(&command)
                    .args(["--json", refresh])
                    .env("PATH", &bin)
                    .env("PROBE_LOG", &log)
                    .output()
                    .unwrap();
                let calls = if log.exists() {
                    std::fs::read_to_string(&log).unwrap()
                } else {
                    String::new()
                };
                if targets.is_empty() {
                    assert!(calls.contains("claude auth status"), "{command:?}: {calls}");
                } else {
                    assert!(calls.is_empty(), "{targets}, {command:?}: {calls}");
                    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
                    let entries = result
                        .get("aliases")
                        .and_then(Value::as_array)
                        .map(|entries| entries.iter().collect::<Vec<_>>())
                        .unwrap_or_else(|| vec![&result]);
                    for entry in entries {
                        assert_ne!(entry["availability"], "runnable", "{entry}");
                        assert_eq!(entry["runnable_paths"], serde_json::json!([]), "{entry}");
                    }
                }
            }
        }
    }
}
