//! Profile fallback membership is independent of the active settings match.

use assert_fs::TempDir;
use serde_json::Value;

use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};

fn assert_profile_falls_back(policies: &str, overrides: &str) {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["claude"]);
    let profile = format!(
        "---\nname: reviewer\nmodel: gpt55\nmodel-policies:\n{policies}\n---\nReview changes."
    );
    let config = format!(
        r#"[settings]
targets = [".claude"]

[models.gpt55]
model = "gpt-5"

[models.gptmini]
model = "gpt-5.4-mini"

[models.sonnet]
model = "claude-opus-4-6"

{overrides}"#
    );
    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", &profile, &[], &config);
    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bundle["routing"]["model_token"], "sonnet");
    assert_eq!(bundle["routing"]["model"], "claude-opus-4-6");
    assert_eq!(bundle["routing"]["harness"], "claude");
    assert_eq!(bundle["execution_policy"]["effort"], "high");
}

#[test]
fn flagged_primary_does_not_veto_other_candidates() {
    assert_profile_falls_back(
        r#"  - match: {alias: gpt55}
    no-fallback: true
  - match: {alias: sonnet}
    override: {effort: high}"#,
        "",
    );
}

#[test]
fn unmatched_primary_override_preserves_profile_candidates() {
    assert_profile_falls_back(
        r#"  - match: {alias: gpt55}
  - match: {alias: sonnet}
    override: {effort: high}"#,
        "[agents.reviewer]\nmodel = \"gptmini\"",
    );
}

#[test]
fn middle_primary_does_not_hide_earlier_candidates() {
    assert_profile_falls_back(
        r#"  - match: {alias: sonnet}
    override: {effort: high}
  - match: {alias: gpt55}"#,
        "",
    );
}

#[test]
fn overlay_settings_match_does_not_remove_profile_candidates() {
    assert_profile_falls_back(
        r#"  - match: {alias: gpt55}
  - match: {alias: sonnet}
    override: {effort: high}"#,
        r#"[[agents.reviewer.model-policies]]
match = {alias = "gpt55"}
no-fallback = true
override = {effort = "low"}"#,
    );
}

#[test]
fn global_settings_match_does_not_remove_profile_candidates() {
    assert_profile_falls_back(
        r#"  - match: {alias: sonnet}
    override: {effort: high}"#,
        r#"[[settings.model-policies]]
match = {alias = "gpt55"}
no-fallback = true
override = {effort = "low"}"#,
    );
}

#[test]
fn literal_candidate_does_not_consume_an_alias_with_the_same_token() {
    assert_profile_falls_back(
        r#"  - match: {alias: gpt55}
  - match: {model: sonnet}
  - match: {alias: sonnet}
    override: {effort: high}"#,
        "",
    );
}

#[test]
fn flagged_entry_does_not_ban_an_unflagged_entry_for_the_same_alias() {
    assert_profile_falls_back(
        r#"  - match: {alias: gpt55}
  - match: {alias: sonnet}
    no-fallback: true
    override: {effort: high}
  - match: {alias: sonnet}
    override: {effort: low}"#,
        "",
    );
}

#[test]
fn inventory_distinguishes_automatic_literal_backups_from_explicit_aliases() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(&temp, &["codex"]);
    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        r#"---
name: reviewer
model: primary
model-policies:
  - match: {alias: primary}
  - match: {model: gpt-5}
    override: {effort: high}
  - match: {alias: gpt-5}
    override: {effort: low}
---
Review changes.
"#,
        &[],
        r#"[settings]
targets = [".codex"]
agent_emission = "never"
[models.primary]
model = "claude-opus-4-6"
[models.gpt-5]
model = "gpt-5.4-mini"
"#,
    );

    // The common catalog contains gpt-5, but not the colliding alias's model.
    std::fs::write(
        project_root.join(".mars/models-cache.json"),
        serde_json::json!({
            "models": [
                {"id": "claude-opus-4-6", "provider": "anthropic"},
                {"id": "gpt-5", "provider": "openai"},
                {"id": "gpt-5.4-mini", "provider": "openai"}
            ],
            "fetched_at": null
        })
        .to_string(),
    )
    .unwrap();

    for (args, expected_model, expected_effort) in [
        (vec![], "gpt-5", "high"),
        (vec!["--model", "gpt-5"], "gpt-5.4-mini", "low"),
    ] {
        let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
        cmd.args([
            "build",
            "launch-bundle",
            "--agent",
            "reviewer",
            "--no-refresh-models",
        ])
        .args(args)
        .env("PATH", replace_path_with(&bin_dir));
        let output = cmd.assert().success().get_output().clone();
        let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(bundle["routing"]["model"], expected_model);
        assert_eq!(bundle["execution_policy"]["effort"], expected_effort);
        let inventory = bundle["prompt_surface"]["inventory_prompt"]
            .as_str()
            .unwrap();
        assert!(
            inventory.contains("Declared backups: primary, gpt-5 (model ID), gpt-5"),
            "{inventory}"
        );
        assert!(inventory.contains("automatic fallback candidates, not native fanout"));
        assert!(inventory.contains("`--model` resolves aliases first"));
        assert!(!inventory.contains("Fan-out:"));
    }
}
