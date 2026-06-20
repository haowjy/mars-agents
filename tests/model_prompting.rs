mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_json::Value;

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

    project.to_path_buf()
}

#[test]
fn models_prompting_json_resolves_agent_first_and_uses_agent_model_guidance() {
    let dir = TempDir::new().unwrap();
    let project = setup_model_prompting_project(&dir);

    let output = mars()
        .args([
            "--json",
            "models",
            "prompting",
            "explorer",
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
