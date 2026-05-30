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

const CODEX_ONLY_AGENT: &str = r#"---
name: explorer
description: Codex only
harness: codex
---
# Explorer
"#;

const DUAL_POLICY_AGENT: &str = r#"---
name: policy-picker
description: First qualifying policy wins
model-policies:
  - match:
      alias: sonnet
    override: {}
  - match:
      alias: opus
    override: {}
---
# Policy Picker
"#;

fn lock_has_native_agent(project: &assert_fs::fixture::ChildPath, agent: &str) -> bool {
    let lock = mars_agents::lock::load(project.path()).expect("load mars.lock");
    lock.contains_output(".claude", &format!("agents/{agent}.md"))
}

fn lock_has_codex_native_agent(project: &assert_fs::fixture::ChildPath, agent: &str) -> bool {
    let lock = mars_agents::lock::load(project.path()).expect("load mars.lock");
    lock.contains_output(".codex", &format!("agents/{agent}.toml"))
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
fn emit_all_first_run_with_claude_target_writes_lowered_native() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]
agent_emission = "always"
"#,
        CLAUDE_HARNESS_AGENT,
        None,
    );

    let native = claude_native_content(&project, "coder");
    assert!(
        !native.contains("harness:"),
        "lowered claude native should drop harness field; got: {native}"
    );
    assert!(
        native.contains("# Coder"),
        "lowered native should retain agent body: {native}"
    );
}

#[test]
fn agent_copy_mixed_selective_only_qualifying_emitted() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "src",
        &[
            ("coder", CLAUDE_HARNESS_AGENT),
            ("explorer", CODEX_ONLY_AGENT),
        ],
        &[],
    );
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

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));

    assert!(project.child(".claude/agents/coder.md").exists());
    assert!(
        !project.child(".claude/agents/explorer.md").exists(),
        "codex-only agent must not be emitted when agent_copy allowlist is claude"
    );
    assert!(
        !project.child(".claude/agents/explorer.toml").exists(),
        "codex native shape must not appear under .claude for non-qualifying agent"
    );
}

#[test]
fn agent_copy_include_fanout_false_blocks_policy_emission() {
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
include_fanout = false

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

    assert!(
        !project.child(".claude/agents/fanout-agent.md").exists(),
        "include_fanout=false must block model-policy qualification"
    );
}

#[test]
fn agent_copy_first_qualifying_policy_wins() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("policy-picker", DUAL_POLICY_AGENT)], &[]);
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

    let native = claude_native_content(&project, "policy-picker");
    assert!(
        native.contains("model: sonnet"),
        "first qualifying model-policy match_value should win: {native}"
    );
    assert!(
        !native.contains("model: opus"),
        "later qualifying policies must not override the first: {native}"
    );
}

#[test]
fn agent_copy_link_fails_on_handwritten_native_collision() {
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

    fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    fs::write(
        project.path().join(".claude/agents/coder.md"),
        "# hand-written native\n",
    )
    .unwrap();

    mars()
        .args([
            "link",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MERIDIAN_MANAGED", "1")
        .assert()
        .failure();

    assert_eq!(
        fs::read_to_string(project.path().join(".claude/agents/coder.md")).unwrap(),
        "# hand-written native\n",
        "link must not overwrite handwritten native agent without --force"
    );
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
fn agent_copy_steady_state_survives_consecutive_syncs() {
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
    assert!(lock_has_native_agent(&project, "coder"));

    sync_project(&project, Some("1"));

    assert!(
        project
            .child(".claude")
            .child("agents")
            .child("coder.md")
            .exists(),
        "second selective sync must not delete native agent emitted on first sync"
    );
    assert!(
        lock_has_native_agent(&project, "coder"),
        "second selective sync must keep native agent lock record"
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

#[test]
fn agent_copy_link_lock_tracks_codex_native_output() {
    const CODEX_AGENT: &str = r#"---
name: explorer
description: Codex explorer
harness: codex
model: gpt-5.3-codex
---
# Explorer
"#;

    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("explorer", CODEX_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = []

[settings.agent_copy]
harnesses = ["codex"]

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));

    mars()
        .args(["link", ".codex", "--root", project.path().to_str().unwrap()])
        .env("MERIDIAN_MANAGED", "1")
        .assert()
        .success();

    assert!(
        project.child(".codex/agents/explorer.toml").exists(),
        "mars link .codex should materialize codex native agent"
    );
    assert!(
        lock_has_codex_native_agent(&project, "explorer"),
        "link should record codex native output against canonical .mars owner"
    );
}

const OPENCODE_HARNESS_AGENT: &str = r#"---
name: integration-tester
description: Runs integration tests
harness: opencode
model: kimi-k2
---
# Integration tester
"#;

#[test]
fn link_opencode_ignores_claude_collision_in_other_target() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "src",
        &[
            ("coder", CLAUDE_HARNESS_AGENT),
            ("integration-tester", OPENCODE_HARNESS_AGENT),
        ],
        &[],
    );
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
targets = []

[settings.agent_copy]
harnesses = ["claude", "opencode"]

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    sync_project(&project, Some("1"));
    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = [\".claude\"]\n")
        .unwrap();

    fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    fs::write(
        project.path().join(".claude/agents/coder.md"),
        "# hand-written native\n",
    )
    .unwrap();

    mars()
        .args([
            "link",
            ".opencode",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MERIDIAN_MANAGED", "1")
        .assert()
        .success();

    assert!(
        project
            .child(".opencode/agents/integration-tester.md")
            .exists(),
        "link .opencode should materialize opencode native agent"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".claude/agents/coder.md")).unwrap(),
        "# hand-written native\n",
        "link .opencode must not touch or overwrite .claude collision"
    );
    assert!(
        !lock_has_native_agent(&project, "coder"),
        "link .opencode must not record .claude native output in lock"
    );
}

#[test]
fn agent_copy_sync_diff_does_not_materialize_native_or_lock() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("coder", CLAUDE_HARNESS_AGENT)], &[]);
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

[dependencies.src]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    fs::create_dir_all(project.path().join(".mars/agents")).unwrap();
    fs::write(
        project.path().join(".mars/agents/coder.md"),
        CLAUDE_HARNESS_AGENT,
    )
    .unwrap();

    mars()
        .args(["sync", "--diff", "--root", project.path().to_str().unwrap()])
        .env("MERIDIAN_MANAGED", "1")
        .assert()
        .success();

    assert!(
        !project.child(".claude/agents/coder.md").exists(),
        "sync --diff must not write selective native agent artifacts"
    );
    let lock_path = project.path().join("mars.lock");
    assert!(
        !lock_path.exists() || !lock_has_native_agent(&project, "coder"),
        "sync --diff must not persist native agent output records in mars.lock"
    );
}

#[test]
fn link_does_not_persist_local_only_target_overlays() {
    let dir = TempDir::new().unwrap();
    let project = setup_with_settings(
        &dir,
        r#"
[settings]
targets = [".claude"]
"#,
        CLAUDE_HARNESS_AGENT,
        None,
    );
    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = [\".claude\", \".codex\"]\n")
        .unwrap();

    mars()
        .args([
            "link",
            ".opencode",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let mars_toml = fs::read_to_string(project.child("mars.toml").path()).unwrap();
    assert!(
        mars_toml.contains(".opencode"),
        "shared mars.toml should include newly linked target"
    );
    assert!(
        !mars_toml.contains(".codex"),
        "shared mars.toml must not persist mars.local.toml-only targets"
    );
}
