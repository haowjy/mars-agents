mod common;
use assert_fs::{TempDir, prelude::*};
use common::*;
use predicates::prelude::*;
use std::fs;

#[test]
fn blocked_self_destination_fails_without_adopting_identical_bytes() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str("[dependencies]\n")
        .unwrap();
    for path in [".mars-src/agents/local.md", ".mars/agents/local.md"] {
        dir.child(path).write_str("# Same bytes\n").unwrap();
    }
    for flags in [vec![], vec!["--force"], vec!["--diff"], vec!["--frozen"]] {
        mars()
            .args([
                "sync",
                "--no-refresh-models",
                "--root",
                dir.path().to_str().unwrap(),
            ])
            .args(flags)
            .assert()
            .failure()
            .stderr(predicate::str::is_match(r"\.mars-src[/\\]agents[/\\]local\.md").unwrap())
            .stderr(predicate::str::is_match(r"\.mars[/\\]agents[/\\]local\.md").unwrap())
            .stderr(predicate::str::contains("relocate"));
        assert_eq!(
            fs::read_to_string(dir.child(".mars/agents/local.md").path()).unwrap(),
            "# Same bytes\n"
        );
        assert!(!dir.child("mars.lock").exists());
    }
}

fn sync(root: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = mars();
    cmd.args([
        "sync",
        "--no-refresh-models",
        "--no-upgrade-hint",
        "--root",
        root.to_str().unwrap(),
    ]);
    cmd
}

fn assert_owner(root: &std::path::Path, owner: &str) {
    let lock: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("mars.lock")).unwrap()).unwrap();
    let item = &lock["items"]["skill/craft"];
    assert_eq!(item["source"].as_str(), Some(owner));
    let outputs = item["outputs"].as_array().unwrap();
    assert_eq!(
        outputs.len(),
        2,
        "one canonical and one native owner: {outputs:?}"
    );
    for target in [".mars", ".codex"] {
        assert_eq!(
            outputs
                .iter()
                .filter(|o| o["target_root"].as_str() == Some(target))
                .count(),
            1
        );
    }
}

#[test]
fn self_layer_transitions_preserve_unique_ownership_and_native_outputs() {
    for same_bytes in [false, true] {
        let dir = TempDir::new().unwrap();
        let dependency = create_source(&dir, "dep", &[], &[("craft", "# Dependency")]);
        let project = dir.child("project");
        let config = format!(
            "[package]\nname = 'demo'\nversion = '1.0.0'\n[settings]\ntargets = ['.codex']\nagent_emission = 'never'\n[dependencies.dep]\npath = '{}'\n",
            portable_path(&dependency)
        );
        project.child("mars.toml").write_str(&config).unwrap();
        sync(project.path()).assert().success();
        assert_owner(project.path(), "dep");
        let package_content = if same_bytes {
            "# Dependency"
        } else {
            "# Package"
        };
        project
            .child("skills/craft/SKILL.md")
            .write_str(package_content)
            .unwrap();
        sync(project.path()).assert().success();
        assert_owner(project.path(), "_self");
        let override_content = if same_bytes {
            "# Dependency"
        } else {
            "# Override"
        };
        project
            .child(".mars-src/skills/craft/SKILL.md")
            .write_str(override_content)
            .unwrap();
        sync(project.path()).assert().success();
        assert_owner(project.path(), "_self");
        assert_eq!(
            fs::read_to_string(project.child(".codex/skills/craft/SKILL.md").path()).unwrap(),
            override_content
        );
        fs::remove_dir_all(project.child(".mars-src/skills/craft").path()).unwrap();
        sync(project.path()).assert().success();
        assert_owner(project.path(), "_self");
        assert_eq!(
            fs::read_to_string(project.child(".codex/skills/craft/SKILL.md").path()).unwrap(),
            package_content
        );
        fs::remove_dir_all(project.child("skills/craft").path()).unwrap();
        sync(project.path()).assert().success();
        assert_owner(project.path(), "dep");
        assert_eq!(
            fs::read_to_string(project.child(".codex/skills/craft/SKILL.md").path()).unwrap(),
            "# Dependency"
        );
    }
}

#[test]
fn canonical_collision_aborts_before_other_outputs_or_lock_change() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str(
            "[package]\nname = 'demo'\nversion = '1.0.0'\n[settings]\nagent_emission = 'never'\n",
        )
        .unwrap();
    dir.child("agents/owned.md")
        .write_str("# Original")
        .unwrap();
    sync(dir.path()).assert().success();
    let lock = fs::read(dir.child("mars.lock").path()).unwrap();
    dir.child("agents/owned.md").write_str("# Update").unwrap();
    dir.child("agents/blocked.md")
        .write_str("# Selected")
        .unwrap();
    dir.child(".mars/agents/blocked.md")
        .write_str("# User")
        .unwrap();
    sync(dir.path()).arg("--json").assert().failure();
    assert_eq!(fs::read(dir.child("mars.lock").path()).unwrap(), lock);
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/owned.md").path()).unwrap(),
        "# Original"
    );
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/blocked.md").path()).unwrap(),
        "# User"
    );
    fs::rename(
        dir.child(".mars/agents/blocked.md").path(),
        dir.child("relocated.md").path(),
    )
    .unwrap();
    sync(dir.path()).assert().success();
    sync(dir.path()).arg("--frozen").assert().success();
}

#[cfg(unix)]
#[test]
fn unowned_self_links_are_refused_without_touching_referents_even_with_force() {
    use std::os::unix::fs::symlink;
    for kind in ["agent", "skill"] {
        for referent in ["source", "outside", "dangling"] {
            let dir = TempDir::new().unwrap();
            let project = dir.child("project");
            project
                .child("mars.toml")
                .write_str("[package]\nname = 'demo'\nversion = '1.0.0'\n")
                .unwrap();
            let (source, dest) = if kind == "agent" {
                ("agents/muse.md", ".mars/agents/muse.md")
            } else {
                ("skills/craft/SKILL.md", ".mars/skills/craft")
            };
            project.child(source).write_str("# Authored").unwrap();
            dir.child("outside/value").write_str("# Outside").unwrap();
            let target = match referent {
                "source" if kind == "agent" => project.child(source).to_path_buf(),
                "source" => project.child("skills/craft").to_path_buf(),
                "outside" if kind == "agent" => dir.child("outside/value").to_path_buf(),
                "outside" => dir.child("outside").to_path_buf(),
                _ => dir.child("absent").to_path_buf(),
            };
            let destination = project.child(dest).to_path_buf();
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            symlink(&target, &destination).unwrap();
            for force in [false, true] {
                let mut command = sync(project.path());
                if force {
                    command.arg("--force");
                }
                command
                    .assert()
                    .failure()
                    .stderr(predicate::str::contains("relocate"));
                assert_eq!(fs::read_link(&destination).unwrap(), target);
                assert_eq!(
                    fs::read_to_string(project.child(source).path()).unwrap(),
                    "# Authored"
                );
                assert_eq!(
                    fs::read_to_string(dir.child("outside/value").path()).unwrap(),
                    "# Outside"
                );
                assert!(!dir.child("absent").exists());
                assert!(!project.child("mars.lock").exists());
            }
        }
    }
}

#[test]
fn self_updates_keep_existing_managed_modification_policy() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str(
            "[package]\nname = 'demo'\nversion = '1.0.0'\n[settings]\nagent_emission = 'never'\n",
        )
        .unwrap();
    dir.child("agents/muse.md").write_str("# Original").unwrap();
    sync(dir.path()).arg("--diff").assert().success();
    assert!(!dir.child("mars.lock").exists());
    assert!(!dir.child(".mars/agents/muse.md").exists());
    sync(dir.path()).arg("--frozen").assert().failure();
    sync(dir.path()).assert().success();
    dir.child(".mars/agents/muse.md")
        .write_str("# Modified output")
        .unwrap();
    sync(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("preserving local content"));
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/muse.md").path()).unwrap(),
        "# Modified output"
    );
    sync(dir.path()).arg("--force").assert().success();
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/muse.md").path()).unwrap(),
        "# Original"
    );
    dir.child(".mars/agents/muse.md")
        .write_str("# Modified again")
        .unwrap();
    dir.child("agents/muse.md")
        .write_str("# Source update")
        .unwrap();
    sync(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("overwriting with upstream"));
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/muse.md").path()).unwrap(),
        "# Source update"
    );
}

#[cfg(unix)]
#[test]
fn self_native_force_replaces_only_selected_link_and_preserves_referent() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.child("mars.toml").write_str("[package]\nname = 'demo'\nversion = '1.0.0'\n[settings]\ntargets = ['.codex']\nagent_emission = 'never'\n").unwrap();
    project
        .child("skills/craft/SKILL.md")
        .write_str("# Selected")
        .unwrap();
    dir.child("outside/SKILL.md")
        .write_str("# User outside")
        .unwrap();
    project.child(".codex/skills").create_dir_all().unwrap();
    symlink(
        dir.child("outside").path(),
        project.child(".codex/skills/craft").path(),
    )
    .unwrap();
    project
        .child(".codex/skills/unrelated/SKILL.md")
        .write_str("# Unrelated")
        .unwrap();
    // Native collisions use the existing partial-outcome policy, not the canonical self gate.
    sync(project.path())
        .assert()
        .stderr(predicate::str::contains("preserved local content"));
    assert!(project.child(".codex/skills/craft").path().is_symlink());
    sync(project.path()).arg("--force").assert().success();
    assert!(!project.child(".codex/skills/craft").path().is_symlink());
    assert_eq!(
        fs::read_to_string(project.child(".codex/skills/craft/SKILL.md").path()).unwrap(),
        "# Selected"
    );
    assert_eq!(
        fs::read_to_string(dir.child("outside/SKILL.md").path()).unwrap(),
        "# User outside"
    );
    assert_eq!(
        fs::read_to_string(project.child(".codex/skills/unrelated/SKILL.md").path()).unwrap(),
        "# Unrelated"
    );
    assert_owner(project.path(), "_self");
}
