// qa-validated: launch-bundle-followup-audit
mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

use common::*;

#[test]
fn sync_diff_does_not_modify_files() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "src", &[("agent", "# Agent content")], &[]);

    let agents_dir = dir.child("project").child(".agents");
    // Manually init so we have the dir without any sync
    fs::create_dir_all(dir.child("project").child(".mars").path()).unwrap();
    fs::write(
        dir.child("project").child("mars.toml").path(),
        format!(
            "[dependencies.src]\npath = \"{}\"\n",
            source.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();

    mars()
        .args([
            "sync",
            "--diff",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // File should NOT be installed (dry run)
    assert!(!agents_dir.child("agents").child("agent.md").exists());
}

#[test]
fn sync_force_overwrites_local_changes() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Original content")], &[]);

    let agents_dir = dir.child("project").child(".agents");
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

    // Locally modify the file
    let installed_file = agents_dir.child("agents").child("coder.md");
    fs::write(installed_file.path(), "# Locally modified").unwrap();

    // Also update source so there's a conflict
    fs::write(source.join("agents").join("coder.md"), "# Upstream update").unwrap();

    // Force sync should overwrite
    mars()
        .args([
            "sync",
            "--force",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = fs::read_to_string(installed_file.path()).unwrap();
    assert_eq!(content, "# Upstream update");
}

#[test]
fn sync_json_includes_target_outcomes() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Coder")], &[]);

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

    let output = mars()
        .args([
            "sync",
            "--json",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();

    let targets = parsed["targets"]
        .as_array()
        .expect("sync --json should include a targets array");
    assert!(!targets.is_empty());
    assert!(targets[0].get("name").is_some());
    assert!(targets[0].get("synced").is_some());
    assert!(targets[0].get("removed").is_some());
}

#[test]
fn sync_materializes_bootstrap_docs_only_to_mars_store_and_removes_cleanly() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Coder")], &[]);
    let bootstrap_dir = source.join("bootstrap/setup");
    fs::create_dir_all(&bootstrap_dir).unwrap();
    fs::write(bootstrap_dir.join("BOOTSTRAP.md"), "# Setup").unwrap();

    mars()
        .args([
            "init",
            ".claude",
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

    let project = dir.child("project");
    assert_eq!(
        fs::read_to_string(project.path().join(".mars/bootstrap/setup/BOOTSTRAP.md")).unwrap(),
        "# Setup"
    );
    assert!(
        !project
            .path()
            .join(".claude/bootstrap/setup/BOOTSTRAP.md")
            .exists(),
        "package-level bootstrap docs must not copy to native harness dirs"
    );

    fs::remove_file(source.join("bootstrap/setup/BOOTSTRAP.md")).unwrap();
    fs::remove_dir(source.join("bootstrap/setup")).unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(
        !project.path().join(".mars/bootstrap/setup").exists(),
        "removed bootstrap docs should clean up their containing directory"
    );
}

#[test]
fn sync_repairs_diverged_native_skill_projection_when_canonical_is_skipped() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[], &[("planning", "# Base")]);
    let variant_dir = source.join("skills/planning/variants/claude");
    fs::create_dir_all(&variant_dir).unwrap();
    fs::write(variant_dir.join("SKILL.md"), "# Claude").unwrap();

    let project = dir.child("project");
    mars()
        .args([
            "init",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let native_skill = project
        .child(".claude")
        .child("skills")
        .child("planning")
        .child("SKILL.md");
    assert_eq!(fs::read_to_string(native_skill.path()).unwrap(), "# Claude");

    fs::write(native_skill.path(), "# Locally edited native projection").unwrap();

    mars()
        .args([
            "sync",
            "--no-upgrade-hint",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: repaired diverged native projection: .claude/skills/planning/SKILL.md",
        ));

    assert_eq!(fs::read_to_string(native_skill.path()).unwrap(), "# Claude");
}

#[test]
fn conflict_flow_with_resolve() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "base",
        &[("coder", "# Original\nline 2\nline 3\n")],
        &[],
    );

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

    // Modify local in .mars/ canonical store (not .agents/ target)
    let mars_installed = dir
        .child("project")
        .child(".mars")
        .child("agents")
        .child("coder.md");
    fs::write(mars_installed.path(), "# Local change\nline 2\nline 3\n").unwrap();

    // Modify source
    fs::write(
        source.join("agents").join("coder.md"),
        "# Upstream change\nline 2\nline 3\n",
    )
    .unwrap();

    // Sync — conflicts now overwrite (source wins) with warning
    mars()
        .args([
            "sync",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicates::str::contains("local modifications"));

    // File in .mars/ should have upstream content (overwritten, no merge markers)
    let content = fs::read_to_string(mars_installed.path()).unwrap();
    assert_eq!(
        content, "# Upstream change\nline 2\nline 3\n",
        "Expected upstream content after overwrite, got: {content}"
    );
}

#[test]
fn add_skips_unmanaged_file_collision() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# Managed coder")], &[]);

    mars()
        .args([
            "init",
            ".agents",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // Place unmanaged file in .mars/ (canonical store) to trigger collision detection
    let mars_dir = dir.child("project").child(".mars");
    let user_file = mars_dir.child("agents").child("coder.md");
    fs::create_dir_all(user_file.path().parent().unwrap()).unwrap();
    fs::write(user_file.path(), "# User-authored").unwrap();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("collides with unmanaged path"));

    let content = fs::read_to_string(user_file.path()).unwrap();
    assert_eq!(content, "# User-authored");

    let lock_content =
        fs::read_to_string(dir.child("project").child("mars.lock").path()).unwrap_or_default();
    assert!(
        !lock_content.contains("agents/coder.md"),
        "collision path should not be added to lock: {lock_content}"
    );
}

#[test]
fn sync_force_overwrites_divergent_target() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "base",
        &[("coder", "# Original\nline 2\nline 3\n")],
        &[],
    );

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

    // Manually edit the target (.agents/) to simulate divergence
    let target_installed = dir
        .child("project")
        .child(".agents")
        .child("agents")
        .child("coder.md");
    fs::write(target_installed.path(), "# Hand-edited content\n").unwrap();

    // Normal sync should warn about divergence but preserve the edit
    mars()
        .args([
            "sync",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(target_installed.path()).unwrap();
    assert_eq!(
        content, "# Hand-edited content\n",
        "Normal sync should preserve local edit"
    );

    // --force should overwrite the divergent target
    mars()
        .args([
            "sync",
            "--force",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();
    let content = fs::read_to_string(target_installed.path()).unwrap();
    assert_eq!(
        content, "# Original\nline 2\nline 3\n",
        "--force should restore canonical content"
    );
}

#[test]
fn sync_frozen_returns_exit_code_two() {
    let dir = TempDir::new().unwrap();
    let source = create_source(&dir, "base", &[("coder", "# v1")], &[]);

    let _agents_dir = dir.child("project").child(".agents");
    mars()
        .args([
            "init",
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

    fs::write(source.join("agents").join("coder.md"), "# v2").unwrap();

    mars()
        .args([
            "sync",
            "--frozen",
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--frozen"));
}

#[test]
fn sync_keeps_canonical_skill_bytes_while_native_target_lowers_invocability_fields() {
    let dir = TempDir::new().unwrap();
    let source_skill = "---\nname: planning\ndescription: base skill\nmodel-invocable: false\nuser-invocable: false\nallowed-tools: [Bash(git *)]\n---\n# Base\n";
    let source = create_source(&dir, "base", &[], &[("planning", source_skill)]);

    let project = dir.child("project");
    mars()
        .args(["init", ".codex", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let canonical_skill = project.child(".mars/skills/planning/SKILL.md");
    assert_eq!(
        fs::read_to_string(canonical_skill.path()).unwrap(),
        source_skill
    );

    let native_skill = project.child(".codex/skills/planning/SKILL.md");
    let native_bytes = fs::read_to_string(native_skill.path()).unwrap();
    assert!(native_bytes.contains("allow_implicit_invocation: false"));
    assert!(!native_bytes.contains("user-invocable"));
    assert!(!native_bytes.contains("allowed-tools"));
    assert_ne!(native_bytes, source_skill);
}

#[test]
fn sync_codex_projection_preserves_explicit_true_and_emits_allow_implicit_invocation_true() {
    let dir = TempDir::new().unwrap();
    let source_skill = "---
name: planning
description: explicit true skill
model-invocable: true
user-invocable: true
---
# Explicit
";
    let source = create_source(&dir, "base", &[], &[("planning", source_skill)]);

    let project = dir.child("project");
    mars()
        .args(["init", ".codex", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let canonical_skill = project.child(".mars/skills/planning/SKILL.md");
    assert_eq!(
        fs::read_to_string(canonical_skill.path()).unwrap(),
        source_skill
    );

    let native_skill = project.child(".codex/skills/planning/SKILL.md");
    let native_bytes = fs::read_to_string(native_skill.path()).unwrap();
    assert!(native_bytes.contains("allow_implicit_invocation: true"));
    assert!(!native_bytes.contains("user-invocable"));
    assert_ne!(native_bytes, source_skill);
}

#[test]
fn sync_codex_projection_omits_allow_implicit_invocation_when_model_invocable_is_absent() {
    let dir = TempDir::new().unwrap();
    let source_skill = "---
name: planning
description: default skill
---
# Default
";
    let source = create_source(&dir, "base", &[], &[("planning", source_skill)]);

    let project = dir.child("project");
    mars()
        .args(["init", ".codex", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let canonical_skill = project.child(".mars/skills/planning/SKILL.md");
    assert_eq!(
        fs::read_to_string(canonical_skill.path()).unwrap(),
        source_skill
    );

    let native_skill = project.child(".codex/skills/planning/SKILL.md");
    let native_bytes = fs::read_to_string(native_skill.path()).unwrap();
    assert!(!native_bytes.contains("allow_implicit_invocation"));
    assert_eq!(native_bytes, source_skill);
}

#[test]
fn sync_native_agent_targets_emit_only_native_agent_artifacts() {
    let dir = TempDir::new().unwrap();
    let codex_agent = r#"---
name: explorer
description: |
  Explorer line one
  Explorer line two
harness: codex
model: gpt-5.3-codex
approval: yolo
sandbox: workspace-write
tools: [Bash, Write, Edit]
---
# Explorer
Use "quotes" and backslashes \\
Keep going.
"#;
    let opencode_agent = r#"---
name: integration-tester
description: Runs integration tests
harness: opencode
model: kimi-k2
tools: [Bash, Write, Edit]
disallowed-tools: [Agent]
---
# Integration tester
Run focused integration checks.
"#;
    let source = create_source(
        &dir,
        "base",
        &[
            ("explorer", codex_agent),
            ("integration-tester", opencode_agent),
        ],
        &[],
    );

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".codex", ".opencode"]
agent_emission = "always"

[dependencies.base]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(project.child(".mars/agents/explorer.md").exists());
    assert!(project.child(".mars/agents/integration-tester.md").exists());

    let codex_agents_dir = project.child(".codex/agents");
    assert!(codex_agents_dir.exists());
    assert!(!project.child(".codex/agents/explorer.md").exists());
    assert!(
        !project
            .child(".codex/agents/integration-tester.md")
            .exists()
    );
    assert!(project.child(".codex/agents/explorer.toml").exists());

    let codex_toml =
        fs::read_to_string(project.child(".codex/agents/explorer.toml").path()).unwrap();
    let parsed: toml::Value = toml::from_str(&codex_toml).expect("codex TOML should parse");
    assert!(parsed.get("agent").is_none(), "legacy [agent] table leaked");
    assert_eq!(
        parsed.get("name").and_then(|v| v.as_str()),
        Some("explorer")
    );
    assert_eq!(
        parsed.get("approval_policy").and_then(|v| v.as_str()),
        Some("never")
    );
    assert!(
        parsed
            .get("developer_instructions")
            .and_then(|v| v.as_str())
            .is_some(),
        "developer_instructions missing"
    );

    assert!(
        project
            .child(".opencode/agents/integration-tester.md")
            .exists()
    );
    assert!(!project.child(".opencode/agents/explorer.md").exists());
    let opencode_native = fs::read_to_string(
        project
            .child(".opencode/agents/integration-tester.md")
            .path(),
    )
    .unwrap();
    assert!(
        !opencode_native.contains("tools:"),
        "native opencode artifact should not include canonical tool lists"
    );
}

#[test]
fn sync_cursor_native_agent_target_emits_cursor_markdown_and_lossiness_warning() {
    let dir = TempDir::new().unwrap();
    let cursor_agent = r#"---
name: cursor-worker
description: Cursor worker
harness: cursor
model: gpt55
harness-overrides:
  opencode:
    mcp-tools: []
  cursor:
    mcp-tools: [plugin:cursor]
    native-config:
      cursor.only: true
---
# Cursor body
Use Cursor-native markdown.
"#;
    let source = create_source(&dir, "base", &[("cursor-worker", cursor_agent)], &[]);

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".cursor"]
agent_emission = "always"

[dependencies.base]
path = "{}"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("mcp-tools"))
        .stderr(predicate::str::contains("Cursor"));

    assert!(project.child(".mars/agents/cursor-worker.md").exists());
    assert!(project.child(".cursor/agents/cursor-worker.md").exists());
    assert!(!project.child(".opencode/agents/cursor-worker.md").exists());
    assert!(!project.child(".codex/agents/cursor-worker.toml").exists());

    let cursor_native =
        fs::read_to_string(project.child(".cursor/agents/cursor-worker.md").path()).unwrap();
    assert!(cursor_native.contains("name: cursor-worker"));
    assert!(cursor_native.contains("model: gpt55"));
    assert!(cursor_native.contains("# Cursor body"));
    assert!(
        !cursor_native.contains("mcp-tools"),
        "Cursor native agent artifacts should not claim native MCP tool support: {cursor_native}"
    );
    assert!(
        !cursor_native.contains("native-config"),
        "native-config is Meridian runtime-only in this slice: {cursor_native}"
    );
    assert!(
        !cursor_native.contains("cursor.only"),
        "native-config keys must not leak into native Cursor markdown: {cursor_native}"
    );
}

#[test]
fn sync_preserves_selected_variant_raw_bytes_when_variant_frontmatter_is_malformed() {
    let dir = TempDir::new().unwrap();
    let base_skill =
        "---\nname: planning\ndescription: base skill\nmodel-invocable: false\n---\n# Base\n";
    let source = create_source(&dir, "base", &[], &[("planning", base_skill)]);
    let malformed_variant =
        "---\nname: ignored\ndescription: malformed variant\nmetadata: [\n---\n# Claude broken\n";
    let variant_dir = source.join("skills/planning/variants/claude");
    fs::create_dir_all(&variant_dir).unwrap();
    fs::write(variant_dir.join("SKILL.md"), malformed_variant).unwrap();

    let project = dir.child("project");
    mars()
        .args([
            "init",
            ".claude",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let add_output = mars()
        .args([
            "add",
            source.to_str().unwrap(),
            "--root",
            project.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        add_output.status.success(),
        "sync with malformed skill frontmatter should still succeed"
    );

    let native_skill = project
        .child(".claude")
        .child("skills")
        .child("planning")
        .child("SKILL.md");
    assert_eq!(
        fs::read_to_string(native_skill.path()).unwrap(),
        malformed_variant,
        "native projection should preserve the raw selected variant bytes"
    );

    assert!(
        String::from_utf8(add_output.stderr)
            .unwrap()
            .contains("selected variant frontmatter is malformed; raw fallback used"),
        "expected sync stderr to report the malformed selected variant fallback"
    );
}

#[test]
fn upgrade_then_sync_keeps_upgraded_transitive_lock_and_content() {
    let dir = TempDir::new().unwrap();

    let shared = create_git_package(
        &dir,
        "shared",
        &[("agents/shared.md", "# Shared v1.0.0\n")],
        "v1.0.0",
    );

    let base_manifest = format!(
        r#"[package]
name = "base"
version = "1.0.0"

[dependencies.shared]
url = "{shared_url}"
version = "^1.0"
"#,
        shared_url = shared.url
    );
    let base = create_git_package(
        &dir,
        "base",
        &[
            ("mars.toml", base_manifest.as_str()),
            ("agents/base.md", "# Base\n"),
        ],
        "v1.0.0",
    );

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[dependencies.base]
url = "{base_url}"
version = "v1.0.0"
"#,
            base_url = base.url
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(
        lock_dependency_version(project.path(), "shared"),
        Some("v1.0.0".to_string())
    );

    add_tagged_release(
        &shared.repo_path,
        "v1.1.0",
        &[("agents/shared.md", "# Shared v1.1.0\n")],
    );
    mars()
        .args(["upgrade", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();
    assert_eq!(
        lock_dependency_version(project.path(), "shared"),
        Some("v1.1.0".to_string()),
        "upgrade should advance transitive lock entry"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".mars/agents/shared.md")).unwrap(),
        "# Shared v1.1.0\n"
    );

    // Introduce a newer tag; plain sync should replay the upgraded lock instead of re-resolving.
    add_tagged_release(
        &shared.repo_path,
        "v1.2.0",
        &[("agents/shared.md", "# Shared v1.2.0\n")],
    );
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    assert_eq!(
        lock_dependency_version(project.path(), "shared"),
        Some("v1.1.0".to_string()),
        "plain sync should retain transitive version selected by upgrade"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".mars/agents/shared.md")).unwrap(),
        "# Shared v1.1.0\n",
        "plain sync should keep installed content from the locked upgraded transitive version"
    );
}

struct GitPackage {
    repo_path: PathBuf,
    url: String,
}

fn create_git_package(
    dir: &TempDir,
    name: &str,
    files: &[(&str, &str)],
    initial_tag: &str,
) -> GitPackage {
    let repo_path = dir.path().join(name);
    fs::create_dir_all(&repo_path).unwrap();
    run_git(&repo_path, &["init", "."]);
    run_git(&repo_path, &["config", "user.name", "Mars Test"]);
    run_git(&repo_path, &["config", "user.email", "mars@example.com"]);

    for (relative_path, content) in files {
        let file_path = repo_path.join(relative_path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(file_path, content).unwrap();
    }

    run_git(&repo_path, &["add", "."]);
    run_git(&repo_path, &["commit", "-m", "initial"]);
    run_git(&repo_path, &["tag", initial_tag]);

    GitPackage {
        url: file_git_url(&repo_path),
        repo_path,
    }
}

fn add_tagged_release(repo_path: &Path, tag: &str, files: &[(&str, &str)]) {
    for (relative_path, content) in files {
        let file_path = repo_path.join(relative_path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(file_path, content).unwrap();
    }

    run_git(repo_path, &["add", "."]);
    run_git(repo_path, &["commit", "-m", tag]);
    run_git(repo_path, &["tag", tag]);
}

fn run_git(cwd: &Path, args: &[&str]) {
    if let Err(err) = mars_agents::platform::process::run_git(args, cwd, "sync-behavior git helper")
    {
        panic!("git command failed: git {}\nerror: {err}", args.join(" "));
    }
}

fn file_git_url(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

fn lock_dependency_version(project_root: &Path, source_name: &str) -> Option<String> {
    let lock_raw = fs::read_to_string(project_root.join("mars.lock")).unwrap();
    let lock: toml::Value = toml::from_str(&lock_raw).unwrap();
    lock.get("dependencies")
        .and_then(|deps| deps.get(source_name))
        .and_then(|dep| dep.get("version"))
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
}
