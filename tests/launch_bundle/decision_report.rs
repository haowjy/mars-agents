//! Selection reports are complete at the CLI boundary, including failed attempts.
use crate::test_common::{install_logging_harnesses, mars_cmd};
use serde_json::Value;
use tempfile::tempdir;

fn launch(settings: &str, local: Option<&str>, args: &[&str]) -> (Value, i32, String) {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = install_logging_harnesses(root);
    std::fs::create_dir_all(root.join(".mars/agents")).unwrap();
    std::fs::write(root.join(".mars/agents/reviewer.md"), "---\nmodel: primary\nmodel-policies:\n  - match: {alias: primary}\n  - match: {alias: backup}\n---\nReview.").unwrap();
    std::fs::write(root.join("mars.toml"), format!("[settings]\n{settings}\n[models.primary]\nmodel=\"claude-opus-4-6\"\n[models.backup]\nmodel=\"gpt-5\"\n")).unwrap();
    if let Some(local) = local {
        std::fs::write(root.join("mars.local.toml"), local).unwrap();
    }
    let log = root.join("calls");
    let output = mars_cmd(root, root, "http://127.0.0.1:1")
        .args([
            "build",
            "launch-bundle",
            "--agent",
            "reviewer",
            "--json",
            "--no-refresh-models",
        ])
        .args(args)
        .env("PATH", bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "not JSON: stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (
        value,
        output.status.code().unwrap(),
        std::fs::read_to_string(log).unwrap_or_default(),
    )
}

#[test]
fn report_records_failed_primary_and_selected_backup_with_local_scope() {
    let (bundle, code, calls) = launch(
        "targets=[\".claude\"]",
        Some("[settings]\ntargets=[\".codex\"]"),
        &[],
    );
    assert_eq!(code, 0, "{bundle}");
    assert_eq!(bundle["version"], 4);
    let report = &bundle["routing"]["route_trace"];
    assert_eq!(report["version"], 2);
    assert_eq!(report["outcome"], "selected");
    assert_eq!(
        report["scope"]["enabled_harnesses"],
        serde_json::json!(["codex"])
    );
    assert_eq!(report["scope"]["target_source"]["origin"], "local");
    assert!(
        report["scope"]["target_source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("mars.local.toml")
    );
    let attempts = report["model_attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["model_token"], "primary");
    assert_eq!(attempts[1]["model_token"], "backup");
    assert_eq!(attempts[1]["canonical_model"], bundle["routing"]["model"]);
    assert_eq!(attempts[1]["model_source"], "profile-model-policy");
    assert_eq!(report["selected"]["attempt_index"], 1);
    let index = report["selected"]["assessment_index"].as_u64().unwrap() as usize;
    assert_eq!(
        attempts[1]["assessments"][index]["harness"],
        bundle["routing"]["harness"]
    );
    assert_eq!(calls.lines().collect::<Vec<_>>(), ["codex login status"]);
}

#[test]
fn report_points_back_to_deferred_primary_after_all_backups() {
    let (bundle, code, calls) = launch("targets=[\".pi\"]", None, &[]);
    assert_eq!(code, 0);
    let report = &bundle["routing"]["route_trace"];
    assert_eq!(report["model_attempts"].as_array().unwrap().len(), 2);
    assert_eq!(report["selected"]["attempt_index"], 0);
    let index = report["selected"]["assessment_index"].as_u64().unwrap() as usize;
    assert_eq!(
        report["model_attempts"][0]["assessments"][index]["verdict"],
        "unverified"
    );
    assert_eq!(bundle["routing"]["model_token"], "primary");
    assert!(calls.is_empty());
}

#[test]
fn json_failures_preserve_attempts_and_explicit_permission_rejection() {
    for (settings, args, outcome, error, count) in [
        (
            "targets=[]",
            vec![],
            "exhausted",
            "model_candidates_exhausted",
            2,
        ),
        (
            "targets=[\".codex\"]",
            vec!["--harness", "claude"],
            "explicit_constraint_error",
            "explicit_harness_excluded",
            1,
        ),
    ] {
        let (value, code, calls) = launch(settings, None, &args);
        assert_ne!(code, 0);
        assert_eq!(value["error"]["code"], error, "{value}");
        let report = &value["route_trace"];
        assert_eq!(report["version"], 2);
        assert_eq!(report["outcome"], outcome);
        assert!(report["selected"].is_null());
        assert_eq!(report["model_attempts"].as_array().unwrap().len(), count);
        assert!(calls.is_empty());
    }
}

#[test]
fn malformed_configuration_has_structured_error_without_fabricated_report() {
    let (value, code, calls) = launch("targets=[", None, &[]);
    assert_ne!(code, 0);
    assert_eq!(value["error"]["code"], "invalid_config");
    assert!(value.get("route_trace").is_none());
    assert!(calls.is_empty());
}

#[test]
fn post_selection_configuration_error_retains_the_selected_report() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = install_logging_harnesses(root);
    std::fs::create_dir_all(root.join(".mars/agents")).unwrap();
    std::fs::write(
        root.join(".mars/agents/broken.md"),
        "---\nmodel: [unterminated\n---\nBroken inventory.",
    )
    .unwrap();
    std::fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".codex\"]\n").unwrap();
    let output = mars_cmd(root, root, "http://127.0.0.1:1")
        .args([
            "build",
            "launch-bundle",
            "--model",
            "gpt-5",
            "--json",
            "--no-refresh-models",
        ])
        .env("PATH", bin)
        .env("PROBE_LOG", root.join("calls"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_config");
    assert_eq!(value["route_trace"]["outcome"], "selected", "{value}");
    assert_eq!(
        value["route_trace"]["model_attempts"][0]["canonical_model"],
        "gpt-5"
    );
    assert!(value.get("routing").is_none());
}
