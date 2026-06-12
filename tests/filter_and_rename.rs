mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::fs;
use toml::Value;

use common::*;

#[test]
fn add_with_agents_filter() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "multi",
        &[
            ("coder", "# Coder"),
            ("reviewer", "# Reviewer"),
            ("planner", "# Planner"),
        ],
        &[],
    );

    // Agents materialize to the canonical `.mars/agents` store; the `.agents`
    // link target only receives native skills.
    let mars_agents_dir = dir.child("project").child(".mars").child("agents");
    mars()
        .args([
            "init",
            ".agents",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--agents",
            "coder",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // Only coder should be installed
    assert!(mars_agents_dir.child("coder.md").exists());
    assert!(!mars_agents_dir.child("reviewer.md").exists());
    assert!(!mars_agents_dir.child("planner.md").exists());
}

#[test]
fn rename_applies_path_mapping_during_sync() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Coder")], &[]);

    // Agents materialize to the canonical `.mars/agents` store.
    let mars_agents_dir = dir.child("project").child(".mars").child("agents");
    mars()
        .args([
            "init",
            ".agents",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "rename",
            "agents/coder.md",
            "agents/coder-renamed.md",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(mars_agents_dir.child("coder-renamed.md").exists());
    assert!(!mars_agents_dir.child("coder.md").exists());

    let lock_content = fs::read_to_string(dir.child("project").child("mars.lock").path()).unwrap();
    let lock: Value = toml::from_str(&lock_content).unwrap();
    assert!(
        lock["items"]
            .as_table()
            .unwrap()
            .contains_key("agent/coder-renamed")
    );
}

#[test]
fn rename_skill_rewrites_agent_skill_references() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "base",
        &[(
            "coder",
            "---\nname: coder\ndescription: test agent\nskills:\n  - planning\n---\n# Coder\n",
        )],
        &[("planning", "# Planning skill")],
    );

    let project_root = dir.child("project");
    let agents_dir = project_root.child(".agents");

    mars()
        .args([
            "init",
            ".agents",
            "--root",
            project_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "rename",
            "skills/planning",
            "skills/strategy",
            "--root",
            project_root.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    agents_dir
        .child("skills")
        .child("strategy")
        .child("SKILL.md")
        .assert(predicate::path::exists());
    agents_dir
        .child("skills")
        .child("planning")
        .assert(predicate::path::missing());

    // Agents materialize to the canonical `.mars/agents` store, not the
    // `.agents` link target (which only receives native skills).
    let mars_agents_dir = project_root.child(".mars").child("agents");
    let agent_content = fs::read_to_string(mars_agents_dir.child("coder.md").path())
        .expect("expected installed agent");
    assert!(
        agent_content.contains("- strategy"),
        "expected renamed skill ref in agent frontmatter, got:\n{agent_content}"
    );
    assert!(
        !agent_content.contains("- planning"),
        "old skill ref should be removed after rename, got:\n{agent_content}"
    );

    mars()
        .args(["doctor", "--root", project_root.path().to_str().unwrap()])
        .assert()
        .success();
}
