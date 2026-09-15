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

#[test]
fn live_models_do_not_advertise_auth_rejected_routes() {
    for fixed in [false, true] {
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
        std::fs::create_dir(root.join(".mars")).unwrap();
        std::fs::write(root.join(".mars/models-cache.json"), r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"},{"id":"gpt-5","provider":"openai"}]}"#).unwrap();
        std::fs::write(root.join("mars.toml"), format!(
            "[settings]\ntargets=[\".claude\",\".codex\"]\n[models.opus]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n{}\n[models.gpt]\nmodel=\"gpt-5\"\nprovider=\"openai\"\n",
            if fixed { "harness=\"claude\"" } else { "" },
        )).unwrap();
        let config = root.join("mars.toml");
        let mut content = std::fs::read_to_string(&config).unwrap();
        content.push_str("[models.uncached]\nmodel=\"claude-uncached\"\nprovider=\"anthropic\"\n");
        std::fs::write(config, content).unwrap();
        for command in [
            vec!["models", "list", "--live", "--unavailable"],
            vec!["models", "list", "--live"],
            vec!["models", "list", "--live", "--all"],
            vec!["models", "list", "--live", "--catalog", "--unavailable"],
            vec!["models", "resolve", "opus"],
            vec!["models", "resolve", "opus-4-6"],
            vec!["models", "resolve", "claude-raw-unknown"],
        ] {
            let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
                .args(&command)
                .args(["--json", "--no-refresh-models"])
                .env("PATH", &bin)
                .env("PROBE_LOG", root.join("commands.log"))
                .output()
                .unwrap();
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            if command[1] == "list" {
                assert!(output.status.success(), "{result}");
                if command.contains(&"--all") || command.contains(&"--catalog") {
                    let entries = result["models"].as_array().unwrap();
                    for entry in entries {
                        if entry["provider"] == "anthropic" {
                            assert_eq!(
                                entry["availability"], "unavailable",
                                "{command:?}: {entry}"
                            );
                            assert_eq!(entry["runnable_paths"], serde_json::json!([]), "{entry}");
                        } else {
                            assert_eq!(entry["availability"], "runnable", "{entry}");
                            assert_eq!(entry["harness"], "codex", "{entry}");
                        }
                    }
                    assert_eq!(
                        entries.len(),
                        if command.contains(&"--all") { 3 } else { 2 }
                    );
                    continue;
                }
                let entries = result["aliases"].as_array().unwrap();
                let gpt = entries.iter().find(|entry| entry["name"] == "gpt").unwrap();
                assert_eq!(gpt["harness"], "codex", "{gpt}");
                assert_eq!(gpt["availability"], "runnable", "{gpt}");
                let opus = entries.iter().find(|entry| entry["name"] == "opus");
                if command.contains(&"--unavailable") {
                    let opus = opus.unwrap();
                    assert_eq!(opus["availability"], "unavailable", "fixed={fixed}: {opus}");
                    assert_eq!(opus["runnable_paths"], serde_json::json!([]), "{opus}");
                    assert_ne!(opus["harness"], "", "{opus}");
                    assert!(
                        !opus["error"].as_str().unwrap().contains("not installed"),
                        "{opus}"
                    );
                } else {
                    assert!(opus.is_none(), "fixed={fixed}: {result}");
                }
            } else {
                assert!(!output.status.success(), "fixed={fixed}: {result}");
                assert_eq!(
                    result["availability"], "unavailable",
                    "fixed={fixed}: {result}"
                );
                assert_eq!(result["runnable_paths"], serde_json::json!([]), "{result}");
                if command[2] == "claude-raw-unknown" {
                    assert_eq!(
                        result["route_rejection"]["reason"], "no_runnable_route",
                        "{result}"
                    );
                    assert!(result["route_rejection"]["harness"].is_null(), "{result}");
                    assert!(
                        !result["error"].as_str().unwrap().contains("not installed"),
                        "{result}"
                    );
                    let text = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
                        .args(&command)
                        .arg("--no-refresh-models")
                        .env("PATH", &bin)
                        .env("PROBE_LOG", root.join("commands.log"))
                        .output()
                        .unwrap();
                    assert!(!text.status.success());
                    assert!(
                        String::from_utf8_lossy(&text.stdout).contains("Availability: unavailable")
                    );
                    assert!(!String::from_utf8_lossy(&text.stderr).contains("not installed"));
                }
            }
        }
    }
}
