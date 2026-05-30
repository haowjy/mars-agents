mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use std::fs;

use common::*;

const CLAUDE_HARNESS_AGENT: &str = r#"---
name: coder
description: Writes code
harness: claude
---
# Coder
"#;

const MODEL_BOUND_AGENT: &str = r#"---
name: reviewer
description: Reviews code
model: opus
---
# Reviewer
"#;

const FANOUT_POLICY_AGENT: &str = r#"---
name: fanout-agent
description: Fanout via policy
model-policies:
  - match:
      alias: sonnet
    override: {}
---
# Fanout Agent
"#;

fn lock_has_native_agent(project: &assert_fs::fixture::ChildPath, agent: &str) -> bool {
    let lock = mars_agents::lock::load(project.path()).expect("load mars.lock");
    lock.contains_output(".claude", &format!("agents/{agent}.md"))
}

fn claude_native_content(project: &assert_fs::fixture::ChildPath, agent: &str) -> String {
    fs::read_to_string(
        project
            .path()
            .join(".claude/agents")
            .join(format!("{agent}.md")),
    )
    .unwrap()
}

fn sync_project(project: &assert_fs::fixture::ChildPath, meridian_managed: Option<&str>) {
    let mut cmd = mars();
    cmd.args(["sync", "--root", project.path().to_str().unwrap()]);
    match meridian_managed {
        Some(value) => {
            cmd.env("MERIDIAN_MANAGED", value);
        }
        None => {
            cmd.env_remove("MERIDIAN_MANAGED");
        }
    }
    cmd.assert().success();
}

fn setup_with_settings(
    dir: &TempDir,
    settings: &str,
    agent_content: &str,
    meridian_managed: Option<&str>,
) -> assert_fs::fixture::ChildPath {
    let source = create_source(dir, "src", &[("coder", agent_content)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    let toml = format!(
        r#"{settings}
[dependencies.src]
path = "{}"
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();
    sync_project(&project, meridian_managed);
    project
}

#[test]
fn agent_copy_emits_claude_native_under_meridian_managed() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]

[settings.agent_copy]
harnesses = ["claude"]
"#,
        CLAUDE_HARNESS_AGENT,
        Some("1"),
    );

    assert!(
        project
            .child(".mars")
            .child("agents")
            .child("coder.md")
            .exists()
    );
    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "agent_copy should emit native claude agent under MERIDIAN_MANAGED"
    );
    assert!(
        !project.child(".agents").exists(),
        "canonical agents should not copy to .agents under selective mode"
    );
}

#[test]
fn agent_copy_supersedes_agent_emission_never() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]
agent_emission = "never"

[settings.agent_copy]
harnesses = ["claude"]
"#,
        CLAUDE_HARNESS_AGENT,
        None,
    );

    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists()
    );
}

#[test]
fn agent_copy_model_binding_qualifies_without_profile_harness() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("reviewer", MODEL_BOUND_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = [".claude"]

[settings.agent_copy]
harnesses = ["claude"]

[models.opus]
model = "claude-opus-4-6"
provider = "anthropic"

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));

    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("reviewer.md")
            .exists()
    );
}

#[test]
fn agent_copy_stale_native_removed_when_config_cleared() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]

[settings.agent_copy]
harnesses = ["claude"]
"#,
        CLAUDE_HARNESS_AGENT,
        Some("1"),
    );
    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists()
    );

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = [".claude"]

[dependencies.src]
path = "{}"
"#,
            dir.child("src")
                .path()
                .display()
                .to_string()
                .replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));

    assert!(
        !project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "removing agent_copy should reconcile stale native agent"
    );
    assert!(
        !lock_has_native_agent(&project, "coder"),
        "stale .claude/agents output record should be removed from mars.lock"
    );
}

#[test]
fn agent_copy_fanout_policy_emits_policy_model_on_claude_native() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("fanout-agent", FANOUT_POLICY_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = [".claude"]

[settings.agent_copy]
harnesses = ["claude"]
include_fanout = true

[models.sonnet]
model = "claude-sonnet-4-6"
provider = "anthropic"

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));

    let native = claude_native_content(&project, "fanout-agent");
    assert!(
        native.contains("model: sonnet"),
        "fanout policy match_value should appear in native claude agent: {native}"
    );
}

#[test]
fn sync_uses_effective_agent_emission_from_local_override() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]
agent_emission = "never"
"#,
        CLAUDE_HARNESS_AGENT,
        Some("1"),
    );
    project
        .child("mars.local.toml")
        .write_str("[settings]\nagent_emission = \"always\"\n")
        .unwrap();
    sync_project(&project, Some("1"));

    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "mars.local.toml agent_emission=always should override project never via effective settings"
    );
}

#[test]
fn agent_copy_link_materializes_selective_native_agents() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("coder", CLAUDE_HARNESS_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = []

[settings.agent_copy]
harnesses = ["claude"]

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));
    assert!(
        !project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "sync without .claude linked should not emit native agent"
    );

    mars()
        .args([
            "link",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MERIDIAN_MANAGED", "1")
        .assert()
        .success();

    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "mars link .claude should materialize selective native agents"
    );
    assert!(
        lock_has_native_agent(&project, "coder"),
        "link should record native agent output in mars.lock"
    );
}
