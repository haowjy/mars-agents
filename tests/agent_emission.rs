mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;

use common::*;

const CLAUDE_AGENT: &str = r#"---
name: coder
description: Writes code
harness: claude
---
# Coder
You write code.
"#;

fn setup_project(
    dir: &TempDir,
    settings: Option<&str>,
    meridian_managed: Option<&str>,
) -> assert_fs::fixture::ChildPath {
    let source = create_source(dir, "src", &[("coder", CLAUDE_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();

    let settings = settings.unwrap_or("");
    let toml = format!(
        r#"{settings}
[dependencies.src]
path = "{}"
"#,
        source.display().to_string().replace('\\', "/")
    );
    project.child("mars.toml").write_str(&toml).unwrap();

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

    project
}

fn assert_canonical_agent_exists(project: &assert_fs::fixture::ChildPath) {
    assert!(
        project
            .child(".mars")
            .child("agents")
            .child("coder.md")
            .exists(),
        "canonical .mars agent should always be emitted"
    );
}

fn native_agent_path(project: &assert_fs::fixture::ChildPath) -> std::path::PathBuf {
    project
        .child(".claude")
        .child("agents")
        .child("coder.md")
        .path()
        .to_path_buf()
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

#[test]
fn default_auto_standalone_emits_native_agent() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(&dir, Some("[settings]\ntargets = [\".claude\"]\n"), None);

    assert_canonical_agent_exists(&project);
    assert!(
        native_agent_path(&project).exists(),
        "standalone auto mode should emit native harness agent"
    );
}

#[test]
fn auto_meridian_managed_suppresses_native_agent() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(&dir, None, Some("1"));

    assert_canonical_agent_exists(&project);
    assert!(
        !native_agent_path(&project).exists(),
        "MERIDIAN_MANAGED=1 auto mode should suppress native harness agent"
    );
}

#[test]
fn always_meridian_managed_still_emits_native_agent() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(
        &dir,
        Some("[settings]\nagent_emission = \"always\"\ntargets = [\".claude\"]\n"),
        Some("1"),
    );

    assert_canonical_agent_exists(&project);
    assert!(
        native_agent_path(&project).exists(),
        "always mode should emit native harness agent even under Meridian"
    );
}

#[test]
fn never_suppresses_native_agent() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(&dir, Some("[settings]\nagent_emission = \"never\"\n"), None);

    assert_canonical_agent_exists(&project);
    assert!(
        !native_agent_path(&project).exists(),
        "never mode should suppress native harness agent"
    );
}

#[test]
fn standalone_sync_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(&dir, Some("[settings]\ntargets = [\".claude\"]\n"), None);

    sync_project(&project, None);

    assert_canonical_agent_exists(&project);
    assert!(
        native_agent_path(&project).exists(),
        "second standalone sync should keep native harness agent"
    );
}

#[test]
fn switching_between_standalone_and_meridian_managed_converges() {
    let dir = TempDir::new().unwrap();
    let project = setup_project(&dir, Some("[settings]\ntargets = [\".claude\"]\n"), None);
    assert!(
        native_agent_path(&project).exists(),
        "standalone sync should emit native harness agent"
    );

    sync_project(&project, Some("1"));
    assert_canonical_agent_exists(&project);
    assert!(
        !native_agent_path(&project).exists(),
        "MERIDIAN_MANAGED sync should remove stale native harness agent"
    );

    sync_project(&project, None);
    assert_canonical_agent_exists(&project);
    assert!(
        native_agent_path(&project).exists(),
        "returning to standalone sync should re-emit native harness agent"
    );
}

fn sync_capture(project: &assert_fs::fixture::ChildPath, meridian_managed: Option<&str>) -> String {
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
    let output = cmd.output().expect("sync runs");
    assert!(output.status.success(), "sync should succeed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn sync_summary_reports_native_emit_and_prune() {
    // Regression (observability gap): the sync summary must surface native-agent
    // emission and removal instead of reporting "already up to date".
    let dir = TempDir::new().unwrap();
    let project = create_emission_project(&dir);

    // Standalone EmitAll: first sync emits the native agent -> summary says "emitted".
    let standalone = sync_capture(&project, None);
    assert!(
        standalone.contains("emitted") && standalone.contains("native agent"),
        "standalone sync should report native emission, got:\n{standalone}"
    );
    assert!(
        !standalone.contains("already up to date"),
        "a sync that emits native agents is not up to date:\n{standalone}"
    );

    // MERIDIAN_MANAGED auto + no agent_copy -> SuppressAll prunes the native agent.
    let managed = sync_capture(&project, Some("1"));
    assert!(
        managed.contains("removed") && managed.contains("native agent"),
        "managed prune should report native removal, got:\n{managed}"
    );
    assert!(
        !managed.contains("already up to date"),
        "a sync that prunes native agents must NOT say up to date:\n{managed}"
    );

    // Steady state: nothing left to prune or emit -> genuinely up to date.
    let steady = sync_capture(&project, Some("1"));
    assert!(
        steady.contains("already up to date"),
        "steady-state managed sync should be up to date, got:\n{steady}"
    );
    assert!(
        !steady.contains("native agent"),
        "steady state should not list native agent changes:\n{steady}"
    );
}

fn create_emission_project(dir: &TempDir) -> assert_fs::fixture::ChildPath {
    let source = create_source(dir, "src", &[("coder", CLAUDE_AGENT)], &[]);
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[settings]\ntargets = [\".claude\"]\n\n[dependencies.src]\npath = \"{}\"\n",
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    project
}
