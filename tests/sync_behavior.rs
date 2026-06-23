// qa-validated: launch-bundle-followup-audit
mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
            "--root",
            dir.child("project").path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // Locally modify the canonical store file
    let installed_file = mars_agents_dir.child("coder.md");
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

    // Manually edit the canonical store to simulate divergence. Agents
    // materialize to `.mars/agents`, not the `.agents` link target.
    let target_installed = dir
        .child("project")
        .child(".mars")
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
    let source_skill = "---\nname: planning\ndescription: base skill\nmodel-invocable: false\nuser-invocable: false\ntools: [Bash(git *)]\n---\n# Base\n";
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
    assert!(!native_bytes.contains("user-invocable"));
    assert!(!native_bytes.contains("allowed-tools"));
    assert_ne!(native_bytes, source_skill);
}

#[test]
fn sync_skill_overlay_stages_canonical_and_lowers_native_target() {
    let dir = TempDir::new().unwrap();
    let source_skill =
        "---\nname: planning\ndescription: base skill\nuser-invocable: true\n---\n# Base\n";
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

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[dependencies.base]
path = "{}"

[skills.planning]
description = "Overridden planning"
user_invocable = false
tools.disallowed = ["Agent"]
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let canonical = fs::read_to_string(
        project
            .child(".mars")
            .child("skills")
            .child("planning")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(canonical.contains("description: Overridden planning"));
    assert!(canonical.contains("user-invocable: false"));
    assert!(canonical.contains("disallowed-tools:"));

    let native = fs::read_to_string(
        project
            .child(".codex")
            .child("skills")
            .child("planning")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(!native.contains("user-invocable"));
    assert!(!native.contains("disallowed-tools"));
    assert_ne!(native, canonical);
}

#[test]
fn sync_mars_local_skill_overlay_overrides_mars_toml() {
    let dir = TempDir::new().unwrap();
    let source_skill = "---\nname: planning\ndescription: base\n---\n# Base\n";
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

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[dependencies.base]
path = "{}"

[skills.planning]
description = "Committed overlay"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    project
        .child("mars.local.toml")
        .write_str(
            r#"
[skills.planning]
description = "Local overlay"
"#,
        )
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    let canonical = fs::read_to_string(
        project
            .child(".mars")
            .child("skills")
            .child("planning")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(canonical.contains("description: Local overlay"));
    assert!(!canonical.contains("Committed overlay"));
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
approval: never
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
    write_cache(
        project.path(),
        vec![json!({
            "id": "gpt-5.5",
            "provider": "OpenAI",
            "release_date": "2026-01-01"
        })],
        &fresh_fetched_at(),
    );
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
    assert!(
        project
            .child(".codex/agents/integration-tester.toml")
            .exists(),
        "EmitAll should emit every agent to every configured harness target"
    );

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
    assert!(
        project.child(".opencode/agents/explorer.md").exists(),
        "EmitAll should emit explorer to opencode even when profile.harness is codex"
    );
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
        .stderr(predicate::str::contains(
            "agent `cursor-worker`: 1 field not lowered (meridian-only) for .cursor (native-config)",
        ));

    assert!(project.child(".mars/agents/cursor-worker.md").exists());
    assert!(project.child(".cursor/agents/cursor-worker.md").exists());
    assert!(!project.child(".opencode/agents/cursor-worker.md").exists());
    assert!(!project.child(".codex/agents/cursor-worker.toml").exists());

    let cursor_native =
        fs::read_to_string(project.child(".cursor/agents/cursor-worker.md").path()).unwrap();
    assert!(cursor_native.contains("name: cursor-worker"));
    assert!(
        !cursor_native.contains("model:"),
        "unresolved cursor model should be cleared in EmitAll mode: {cursor_native}"
    );
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
fn sync_cursor_native_agent_maps_alias_to_internal_cursor_model_slug() {
    let dir = TempDir::new().unwrap();
    let cache_root = dir.child("mars-cache");
    write_cursor_probe_cache(cache_root.path(), &["gpt-5.5-high", "gpt-5.5-low"]);
    let cursor_agent = r#"---
name: cursor-worker
description: Cursor worker
harness: cursor
model: gpt55
---
# Cursor body
Use Cursor-native markdown.
"#;
    let source = create_source(&dir, "base", &[("cursor-worker", cursor_agent)], &[]);
    fs::write(
        source.join("mars.toml"),
        r#"[package]
name = "base"
version = "0.1.0"

[models.gpt55]
harness = "codex"
model = "gpt-5.5"
default_effort = "high"
"#,
    )
    .unwrap();

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
        .args([
            "sync",
            "--no-refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MARS_CACHE_DIR", cache_root.path())
        .assert()
        .success();

    let cursor_native =
        fs::read_to_string(project.child(".cursor/agents/cursor-worker.md").path()).unwrap();
    assert!(
        cursor_native.contains("model: gpt-5.5-high"),
        "cursor model should be internally adapted: {cursor_native}"
    );
    assert!(
        !cursor_native.contains("model: gpt55"),
        "alias token should not leak into cursor native file when mapping exists: {cursor_native}"
    );
}

#[test]
fn sync_cursor_native_agent_uses_mars_local_model_overlay_for_native_slug_mapping() {
    let dir = TempDir::new().unwrap();
    let cache_root = dir.child("mars-cache");
    write_cursor_probe_cache(cache_root.path(), &["gpt-5.5-high", "gpt-5.5-turbo-high"]);
    let cursor_agent = r#"---
name: cursor-worker
description: Cursor worker
harness: cursor
model: gpt55
---
# Cursor body
Use Cursor-native markdown.
"#;
    let source = create_source(&dir, "base", &[("cursor-worker", cursor_agent)], &[]);
    fs::write(
        source.join("mars.toml"),
        r#"[package]
name = "base"
version = "0.1.0"

[models.gpt55]
harness = "codex"
model = "gpt-5.5"
default_effort = "high"
"#,
    )
    .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    write_cache(
        project.path(),
        vec![
            json!({
                "id": "gpt-5.5",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
            json!({
                "id": "gpt-5.5-turbo",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
        ],
        &fresh_fetched_at(),
    );
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
    project
        .child("mars.local.toml")
        .write_str(
            r#"[models.gpt55]
harness = "codex"
model = "gpt-5.5-turbo"
default_effort = "high"
"#,
        )
        .unwrap();

    mars()
        .args([
            "sync",
            "--no-refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .env("MARS_CACHE_DIR", cache_root.path())
        .assert()
        .success();

    let cursor_native =
        fs::read_to_string(project.child(".cursor/agents/cursor-worker.md").path()).unwrap();
    assert!(
        cursor_native.contains("model: gpt-5.5-turbo-high"),
        "local model overlay should drive cursor slug mapping: {cursor_native}"
    );
    assert!(
        !cursor_native.contains("model: gpt-5.5-high"),
        "dependency alias model should be overridden by mars.local.toml: {cursor_native}"
    );
}

#[test]
fn sync_suppresses_dependency_model_alias_conflict_when_local_overlay_defines_alias() {
    let dir = TempDir::new().unwrap();

    let dep_a = create_source(&dir, "dep-a", &[("a", "# A\n")], &[]);
    fs::write(
        dep_a.join("mars.toml"),
        r#"[package]
name = "dep-a"
version = "0.1.0"

[models.shared]
harness = "codex"
model = "gpt-5"
"#,
    )
    .unwrap();

    let dep_b = create_source(&dir, "dep-b", &[("b", "# B\n")], &[]);
    fs::write(
        dep_b.join("mars.toml"),
        r#"[package]
name = "dep-b"
version = "0.1.0"

[models.shared]
harness = "codex"
model = "gpt-5.1"
"#,
    )
    .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[dependencies.dep-a]
path = "{}"

[dependencies.dep-b]
path = "{}"
"#,
            dep_a.display().to_string().replace('\\', "/"),
            dep_b.display().to_string().replace('\\', "/")
        ))
        .unwrap();
    project
        .child("mars.local.toml")
        .write_str(
            r#"[models.shared]
harness = "codex"
model = "gpt-5-local"
"#,
        )
        .unwrap();

    let output = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sync should succeed, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("model-alias-conflict"),
        "diagnostic code should be suppressed when local overlay owns the alias: {stderr}"
    );
    assert!(
        !stderr.contains("model alias `shared` defined by both"),
        "local [models.shared] overlay should suppress dependency alias conflict diagnostics: {stderr}"
    );
}

#[test]
fn sync_persists_dependency_model_aliases_in_consumer_declaration_order() {
    let dir = TempDir::new().unwrap();
    let dep_a = create_source(&dir, "dep-a", &[("a", "# A\n")], &[]);
    fs::write(
        dep_a.join("mars.toml"),
        r#"[package]
name = "dep-a"
version = "0.1.0"

[models.shared]
harness = "codex"
model = "gpt-a"
"#,
    )
    .unwrap();
    let dep_b = create_source(&dir, "dep-b", &[("b", "# B\n")], &[]);
    fs::write(
        dep_b.join("mars.toml"),
        r#"[package]
name = "dep-b"
version = "0.1.0"

[models.shared]
harness = "codex"
model = "gpt-b"
"#,
    )
    .unwrap();

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[dependencies.dep-b]
path = "{}"

[dependencies.dep-a]
path = "{}"
"#,
            dep_b.display().to_string().replace('\\', "/"),
            dep_a.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args([
            "sync",
            "--no-refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    write_cache(
        project.path(),
        vec![
            json!({
                "id": "gpt-a",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
            json!({
                "id": "gpt-b",
                "provider": "OpenAI",
                "release_date": "2026-01-01"
            }),
        ],
        &fresh_fetched_at(),
    );
    let output = mars()
        .args([
            "--json",
            "models",
            "list",
            "--unavailable",
            "--no-refresh-models",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "models list should succeed, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let shared = stdout["aliases"]
        .as_array()
        .expect("models list should include aliases")
        .iter()
        .find(|alias| alias["name"].as_str() == Some("shared"))
        .expect("shared dependency alias should be listed");
    assert_eq!(
        shared["model_id"].as_str(),
        Some("gpt-b"),
        "the dependency declared first in mars.toml should win persisted alias conflicts: {stdout}"
    );
}

fn write_cursor_probe_cache(cache_root: &Path, slugs: &[&str]) {
    let cache_path = cache_root.join("availability").join("cursor-probe.json");
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_secs();
    let payload = json!({
        "schema_version": 1,
        "fetched_at": now,
        "last_attempt_at": now,
        "last_error": null,
        "result": {
            "slugs": slugs,
            "model_probe_success": true,
            "error": null
        }
    });
    fs::write(cache_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
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
        fs::read_to_string(project.path().join(".mars/agents/shared.md"))
            .unwrap()
            .replace("\r\n", "\n"),
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
        .success()
        .stderr(predicate::str::contains("1 upgrade available"))
        .stderr(predicate::str::contains("mars upgrade"));

    assert_eq!(
        lock_dependency_version(project.path(), "shared"),
        Some("v1.1.0".to_string()),
        "plain sync should retain transitive version selected by upgrade"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".mars/agents/shared.md"))
            .unwrap()
            .replace("\r\n", "\n"),
        "# Shared v1.1.0\n",
        "plain sync should keep installed content from the locked upgraded transitive version"
    );
}

#[test]
fn upgrade_bump_mutates_only_direct_mars_toml_dependencies() {
    let dir = TempDir::new().unwrap();

    let shared = create_git_package(
        &dir,
        "shared",
        &[("agents/shared.md", "# Shared v1.0.0\n")],
        "v1.0.0",
    );

    let base_manifest_v1 = format!(
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
            ("mars.toml", base_manifest_v1.as_str()),
            ("agents/base.md", "# Base v1.0.0\n"),
        ],
        "v1.0.0",
    );

    add_tagged_release(
        &shared.repo_path,
        "v1.1.0",
        &[("agents/shared.md", "# Shared v1.1.0\n")],
    );

    let base_manifest_v1_1 = format!(
        r#"[package]
name = "base"
version = "1.1.0"

[dependencies.shared]
url = "{shared_url}"
version = "^1.0"
"#,
        shared_url = shared.url
    );
    add_tagged_release(
        &base.repo_path,
        "v1.1.0",
        &[
            ("mars.toml", base_manifest_v1_1.as_str()),
            ("agents/base.md", "# Base v1.1.0\n"),
        ],
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

    mars()
        .args([
            "upgrade",
            "--bump",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let mars_toml_raw = fs::read_to_string(project.path().join("mars.toml")).unwrap();
    let mars_toml: toml::Value = toml::from_str(&mars_toml_raw).unwrap();
    let dependencies = mars_toml
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .expect("mars.toml should contain dependencies table");

    assert_eq!(
        dependencies
            .get("base")
            .and_then(|entry| entry.get("version"))
            .and_then(toml::Value::as_str),
        Some("v1.1.0"),
        "upgrade --bump should update direct dependency constraints to resolved tags"
    );
    assert!(
        dependencies.get("shared").is_none(),
        "upgrade --bump must not add transitive-only dependencies to mars.toml"
    );
    assert_eq!(
        lock_dependency_version(project.path(), "shared"),
        Some("v1.1.0".to_string()),
        "shared remains transitive and locked without becoming a direct mars.toml dependency"
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

#[test]
fn sync_preserves_handwritten_cursor_agents_when_lock_only_tracks_mars() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "base",
        &[("design-lead", "# Design lead from Mars")],
        &[],
    );

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            r#"[settings]
targets = [".cursor"]
agent_emission = "never"

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

    fs::create_dir_all(project.child(".cursor/agents").path()).unwrap();
    fs::write(
        project.child(".cursor/agents/cursor-only-test.md").path(),
        "# custom\n",
    )
    .unwrap();
    fs::write(
        project.child(".cursor/agents/design-lead.md").path(),
        "# hand-written\n",
    )
    .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(project.child(".cursor/agents/cursor-only-test.md").exists());
    assert!(project.child(".cursor/agents/design-lead.md").exists());
    assert_eq!(
        fs::read_to_string(project.child(".cursor/agents/design-lead.md").path()).unwrap(),
        "# hand-written\n"
    );
}

#[test]
fn sync_preserves_handwritten_cursor_agent_with_agent_emission_always() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "base",
        &[("design-lead", "# Design lead from Mars")],
        &[],
    );

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
        .success();

    // A genuinely unmanaged, user-authored agent uses a name mars does not emit.
    // Under full-coverage EmitAll, mars owns every *source* agent name on every
    // configured target, so an edit to design-lead.md is managed drift that sync
    // restores. Unmanaged-collision protection only guards non-source names.
    fs::create_dir_all(project.child(".cursor/agents").path()).unwrap();
    fs::write(
        project.child(".cursor/agents/handwritten-only.md").path(),
        "# hand-written\n",
    )
    .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    // The unmanaged file survives untouched.
    assert_eq!(
        fs::read_to_string(project.child(".cursor/agents/handwritten-only.md").path()).unwrap(),
        "# hand-written\n"
    );
    // And full-coverage EmitAll still emits the mars source agent to the target.
    assert!(project.child(".cursor/agents/design-lead.md").exists());
}

#[test]
fn link_fails_on_unmanaged_collision_without_force() {
    let dir = TempDir::new().unwrap();
    // Skills are the only items emitted to the `.agents` link target now;
    // agents materialize to the canonical `.mars/agents` store. Test the
    // unmanaged-collision guard against a skill the link target would touch.
    let source = create_source(&dir, "base", &[], &[("planning", "# Planning from Mars")]);

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n",
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    fs::create_dir_all(project.child(".agents/skills/planning").path()).unwrap();
    fs::write(
        project.child(".agents/skills/planning/SKILL.md").path(),
        "# hand-written\n",
    )
    .unwrap();

    mars()
        .args([
            "link",
            ".agents",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .failure();

    assert_eq!(
        fs::read_to_string(project.child(".agents/skills/planning/SKILL.md").path()).unwrap(),
        "# hand-written\n"
    );
}

#[test]
fn link_force_adopts_unmanaged_collision_and_records_lock() {
    let dir = TempDir::new().unwrap();
    // Skills are the only items emitted to the `.agents` link target now;
    // agents materialize to the canonical `.mars/agents` store. Test
    // `--force` adoption against a skill the link target would touch.
    let source = create_source(&dir, "base", &[], &[("planning", "# Planning from Mars")]);

    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!(
            "[dependencies.base]\npath = \"{}\"\n",
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
        .success();

    fs::create_dir_all(project.child(".agents/skills/planning").path()).unwrap();
    fs::write(
        project.child(".agents/skills/planning/SKILL.md").path(),
        "# hand-written\n",
    )
    .unwrap();

    mars()
        .args([
            "link",
            ".agents",
            "--force",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(project.child(".agents/skills/planning/SKILL.md").path()).unwrap(),
        "# Planning from Mars"
    );

    let lock = mars_agents::lock::load(project.path()).unwrap();
    assert!(lock.contains_output(".agents", "skills/planning"));
}

#[test]
fn sync_mars_native_dependency_skill_strips_allowed_tools_and_warns() {
    let dir = TempDir::new().unwrap();
    let source_skill =
        "---\nname: bad-allowed\ndescription: d\nallowed-tools: [Bash]\n---\n# Body\n";
    let source = create_source(&dir, "base", &[], &[("bad-allowed", source_skill)]);

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

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
managed_root = ".claude"

[dependencies.base]
path = "{}"
dialect = "mars-native"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let sync = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "sync failed: {:?}\nstderr: {}",
        sync.status,
        String::from_utf8_lossy(&sync.stderr)
    );

    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        stderr.contains("allowed-tools") && stderr.contains("tools:"),
        "sync must warn about non-canonical allowed-tools: {stderr}"
    );

    let canonical = fs::read_to_string(
        project
            .child(".mars")
            .child("skills")
            .child("bad-allowed")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !canonical.contains("allowed-tools"),
        "canonical store must not retain allowed-tools: {canonical}"
    );

    let claude_native = fs::read_to_string(
        project
            .child(".claude")
            .child("skills")
            .child("bad-allowed")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !claude_native.contains("allowed-tools"),
        "claude projection must not leak allowed-tools: {claude_native}"
    );
}

#[test]
fn sync_local_mars_native_skill_strips_allowed_tools_and_warns() {
    let dir = TempDir::new().unwrap();
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

    let local_skill = project
        .child(".mars-src")
        .child("skills")
        .child("bad-allowed");
    fs::create_dir_all(local_skill.path()).unwrap();
    fs::write(
        local_skill.child("SKILL.md").path(),
        "---\nname: bad-allowed\ndescription: d\nallowed-tools: [Bash]\n---\n# Body\n",
    )
    .unwrap();

    let sync = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "sync failed: {:?}\nstderr: {}",
        sync.status,
        String::from_utf8_lossy(&sync.stderr)
    );

    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        stderr.contains("allowed-tools") && stderr.contains("tools:"),
        "sync must warn about non-canonical allowed-tools for local skill: {stderr}"
    );

    let canonical = fs::read_to_string(
        project
            .child(".mars")
            .child("skills")
            .child("bad-allowed")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !canonical.contains("allowed-tools"),
        "canonical store must not retain allowed-tools: {canonical}"
    );

    let claude_native = fs::read_to_string(
        project
            .child(".claude")
            .child("skills")
            .child("bad-allowed")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !claude_native.contains("allowed-tools"),
        "claude projection must not leak allowed-tools: {claude_native}"
    );
}

#[test]
fn sync_mars_native_dependency_skill_strips_disallowed_tools_and_warns() {
    let dir = TempDir::new().unwrap();
    let source_skill =
        "---\nname: bad-denied\ndescription: d\ndisallowed_tools: [Agent]\n---\n# Body\n";
    let source = create_source(&dir, "base", &[], &[("bad-denied", source_skill)]);

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

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
managed_root = ".claude"

[dependencies.base]
path = "{}"
dialect = "mars-native"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let sync = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "sync failed: {:?}\nstderr: {}",
        sync.status,
        String::from_utf8_lossy(&sync.stderr)
    );

    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        stderr.contains("disallowed_tools") && stderr.contains("disallowed-tools:"),
        "sync must warn about non-canonical disallowed_tools: {stderr}"
    );

    let canonical = fs::read_to_string(
        project
            .child(".mars")
            .child("skills")
            .child("bad-denied")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !canonical.contains("disallowed_tools"),
        "canonical store must not retain disallowed_tools: {canonical}"
    );

    let claude_native = fs::read_to_string(
        project
            .child(".claude")
            .child("skills")
            .child("bad-denied")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        !claude_native.contains("disallowed_tools"),
        "claude projection must not leak disallowed_tools: {claude_native}"
    );
}

#[test]
fn sync_mars_native_canonical_tools_skill_lowers_without_non_canonical_warning() {
    let dir = TempDir::new().unwrap();
    let source_skill = "---\nname: good-tools\ndescription: d\ntools: [Bash]\n---\n# Body\n";
    let source = create_source(&dir, "base", &[], &[("good-tools", source_skill)]);

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

    project
        .child("mars.toml")
        .write_str(&format!(
            r#"
[settings]
managed_root = ".claude"

[dependencies.base]
path = "{}"
dialect = "mars-native"
"#,
            source.display().to_string().replace('\\', "/")
        ))
        .unwrap();

    let sync = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "sync failed: {:?}\nstderr: {}",
        sync.status,
        String::from_utf8_lossy(&sync.stderr)
    );

    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        !stderr.contains("skill-schema-warning"),
        "canonical tools must not emit non-canonical warning: {stderr}"
    );

    let claude_native = fs::read_to_string(
        project
            .child(".claude")
            .child("skills")
            .child("good-tools")
            .child("SKILL.md")
            .path(),
    )
    .unwrap();
    assert!(
        claude_native.contains("allowed-tools:"),
        "canonical tools should lower to claude allowed-tools: {claude_native}"
    );
}
