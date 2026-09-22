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
    let source_before = fs::read(dir.child(".mars-src/agents/local.md").path()).unwrap();
    let canonical_before = fs::read(dir.child(".mars/agents/local.md").path()).unwrap();

    for flags in [vec![], vec!["--diff"], vec!["--frozen"]] {
        sync(dir.path())
            .args(flags)
            .assert()
            .failure()
            .stderr(predicate::str::is_match(r"\.mars-src[/\\]agents[/\\]local\.md").unwrap())
            .stderr(predicate::str::is_match(r"\.mars[/\\]agents[/\\]local\.md").unwrap())
            .stderr(predicate::str::contains("mars sync --force"));
        assert_eq!(
            fs::read(dir.child(".mars-src/agents/local.md").path()).unwrap(),
            source_before
        );
        assert_eq!(
            fs::read(dir.child(".mars/agents/local.md").path()).unwrap(),
            canonical_before
        );
        assert!(!dir.child("mars.lock").exists());
    }
}

#[test]
fn force_adopts_unowned_self_agent_and_skill_outputs() {
    use mars_agents::{hash, lock};

    for (layer, source_root) in [("package", ""), ("mars-src", ".mars-src/")] {
        for (kind_name, source_rel, dest_rel, item_kind) in [
            (
                "agent",
                "agents/muse.md",
                ".mars/agents/muse.md",
                lock::ItemKind::Agent,
            ),
            (
                "skill",
                "skills/craft/SKILL.md",
                ".mars/skills/craft",
                lock::ItemKind::Skill,
            ),
        ] {
            for same_bytes in [true, false] {
                let dir = TempDir::new().unwrap();
                let config = if layer == "package" {
                    "[package]\nname = 'demo'\nversion = '1.0.0'\n"
                } else {
                    "[dependencies]\n"
                };
                dir.child("mars.toml").write_str(config).unwrap();
                let source = dir.child(format!("{source_root}{source_rel}"));
                let original = if same_bytes {
                    "# Same\n"
                } else {
                    "# Existing\n"
                };
                let desired = if same_bytes { original } else { "# Desired\n" };
                source.write_str(desired).unwrap();
                let destination = dir.child(dest_rel);
                if item_kind == lock::ItemKind::Agent {
                    destination.write_str(original).unwrap();
                } else {
                    destination.child("SKILL.md").write_str(original).unwrap();
                }
                dir.child(".mars/unrelated.txt")
                    .write_str("sentinel\n")
                    .unwrap();

                let source_before = fs::read(source.path()).unwrap();
                let canonical_file = if item_kind == lock::ItemKind::Agent {
                    destination.path().to_path_buf()
                } else {
                    destination.child("SKILL.md").path().to_path_buf()
                };
                let canonical_before = fs::read(&canonical_file).unwrap();

                sync(dir.path()).assert().failure();
                assert_eq!(fs::read(source.path()).unwrap(), source_before);
                assert_eq!(fs::read(&canonical_file).unwrap(), canonical_before);
                assert_eq!(
                    fs::read(dir.child(".mars/unrelated.txt").path()).unwrap(),
                    b"sentinel\n"
                );
                assert!(!dir.child("mars.lock").exists());

                sync(dir.path())
                    .arg("--force")
                    .arg("--diff")
                    .assert()
                    .success()
                    .stdout(predicate::str::contains("would install"))
                    .stderr(predicate::str::contains("would replace and adopt"));
                assert_eq!(fs::read(source.path()).unwrap(), source_before);
                assert_eq!(fs::read(&canonical_file).unwrap(), canonical_before);
                assert_eq!(
                    fs::read(dir.child(".mars/unrelated.txt").path()).unwrap(),
                    b"sentinel\n"
                );
                assert!(!dir.child("mars.lock").exists());

                sync(dir.path())
                    .arg("--force")
                    .assert()
                    .success()
                    .stdout(predicate::str::contains("installed"))
                    .stderr(predicate::str::contains("will replace and adopt"));
                assert_eq!(fs::read(&canonical_file).unwrap(), desired.as_bytes());
                assert_eq!(fs::read(source.path()).unwrap(), source_before);
                assert_eq!(
                    fs::read(dir.child(".mars/unrelated.txt").path()).unwrap(),
                    b"sentinel\n"
                );

                let installed_lock = lock::load(dir.path()).unwrap();
                let item_key = format!(
                    "{kind_name}/{}",
                    if kind_name == "agent" {
                        "muse"
                    } else {
                        "craft"
                    }
                );
                let item = installed_lock
                    .items
                    .get(&item_key)
                    .unwrap_or_else(|| panic!("missing exact lock item {item_key}"));
                assert_eq!(item.source.as_str(), "_self");
                assert_eq!(item.kind, item_kind);
                let canonical_output = item
                    .outputs
                    .iter()
                    .find(|output| {
                        output.target_root == ".mars"
                            && output.dest_path.as_str() == dest_rel.strip_prefix(".mars/").unwrap()
                    })
                    .unwrap_or_else(|| panic!("missing canonical output for {item_key}"));
                let expected_hash = hash::compute_hash(destination.path(), item_kind).unwrap();
                assert_eq!(
                    canonical_output
                        .installed_checksum()
                        .expect("canonical output must be installed")
                        .as_str(),
                    expected_hash
                );

                let lock_before_repeat = fs::read(dir.child("mars.lock").path()).unwrap();
                sync(dir.path())
                    .arg("--force")
                    .assert()
                    .success()
                    .stdout(predicate::str::contains("already up to date"))
                    .stderr(predicate::str::contains("self-adopt").not());
                assert_eq!(
                    fs::read(dir.child("mars.lock").path()).unwrap(),
                    lock_before_repeat
                );
            }
        }
    }
}

#[test]
fn force_frozen_only_succeeds_after_adoption_is_current() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str("[dependencies]\n")
        .unwrap();
    dir.child(".mars-src/agents/muse.md")
        .write_str("# Selected\n")
        .unwrap();
    dir.child(".mars/agents/muse.md")
        .write_str("# Authored\n")
        .unwrap();
    let source_before = fs::read(dir.child(".mars-src/agents/muse.md").path()).unwrap();
    let canonical_before = fs::read(dir.child(".mars/agents/muse.md").path()).unwrap();

    sync(dir.path())
        .args(["--force", "--frozen"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("will replace and adopt").not());
    assert_eq!(
        fs::read(dir.child(".mars-src/agents/muse.md").path()).unwrap(),
        source_before
    );
    assert_eq!(
        fs::read(dir.child(".mars/agents/muse.md").path()).unwrap(),
        canonical_before
    );
    assert!(!dir.child("mars.lock").exists());

    sync(dir.path()).arg("--force").assert().success();
    let lock_before = fs::read(dir.child("mars.lock").path()).unwrap();
    sync(dir.path())
        .args(["--force", "--frozen"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already up to date"))
        .stderr(predicate::str::contains("self-adopt").not());
    assert_eq!(
        fs::read(dir.child("mars.lock").path()).unwrap(),
        lock_before
    );
}

#[cfg(unix)]
#[test]
fn force_rejects_symlinked_canonical_root_ancestors_and_destination() {
    use std::os::unix::fs::symlink;

    for (kind, link_level) in [
        ("agent", "root-in-project"),
        ("agent", "root-outside"),
        ("agent", "ancestor"),
        ("skill", "root-in-project"),
        ("skill", "root-outside"),
        ("skill", "ancestor"),
        ("skill", "destination"),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project
            .child("mars.toml")
            .write_str("[dependencies]\n")
            .unwrap();
        let (source, relative) = if kind == "agent" {
            (".mars-src/agents/muse.md", "agents/muse.md")
        } else {
            (".mars-src/skills/craft/SKILL.md", "skills/craft")
        };
        project.child(source).write_str("# Selected\n").unwrap();
        let authored = if link_level == "root-in-project" {
            project.child("authored")
        } else {
            dir.child("authored")
        };
        let referent_file = if kind == "agent" {
            authored.child(relative)
        } else {
            authored.child(relative).child("SKILL.md")
        };
        referent_file.write_str("# Authored\n").unwrap();
        let canonical = project.child(".mars");
        match link_level {
            "root-in-project" | "root-outside" => {
                symlink(authored.path(), canonical.path()).unwrap()
            }
            "ancestor" => {
                fs::create_dir_all(canonical.path()).unwrap();
                let component = if kind == "agent" { "agents" } else { "skills" };
                symlink(
                    authored.child(component).path(),
                    canonical.child(component).path(),
                )
                .unwrap();
            }
            "destination" => {
                let destination = canonical.join(relative);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                symlink(authored.join(relative), destination).unwrap();
            }
            _ => unreachable!(),
        }
        sync(project.path())
            .arg("--force")
            .assert()
            .failure()
            .stderr(predicate::str::contains("will replace and adopt").not());
        assert_eq!(
            fs::read_to_string(referent_file.path()).unwrap(),
            "# Authored\n"
        );
        assert!(!project.child("mars.lock").exists());
    }
}

#[cfg(unix)]
#[test]
fn force_replaces_skill_tree_without_following_nested_destination_link() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str("[dependencies]\n")
        .unwrap();
    dir.child(".mars-src/skills/craft/SKILL.md")
        .write_str("# Selected\n")
        .unwrap();
    let destination = dir.child(".mars/skills/craft");
    destination
        .child("SKILL.md")
        .write_str("# Authored\n")
        .unwrap();
    let referent = dir.child("outside/keep.txt");
    referent.write_str("keep\n").unwrap();
    symlink(referent.path(), destination.child("nested-link").path()).unwrap();

    sync(dir.path()).arg("--force").assert().success();

    assert_eq!(
        fs::read_to_string(destination.child("SKILL.md").path()).unwrap(),
        "# Selected\n"
    );
    assert!(!destination.child("nested-link").exists());
    assert_eq!(fs::read_to_string(referent.path()).unwrap(), "keep\n");
}

#[test]
fn failed_force_adoption_does_not_publish_ownership_and_retry_requires_force() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str("[dependencies]\n")
        .unwrap();
    dir.child(".mars-src/agents/muse.md")
        .write_str("# Selected agent\n")
        .unwrap();
    dir.child(".mars-src/skills/craft/SKILL.md")
        .write_str("# Selected skill\n")
        .unwrap();
    dir.child(".mars/agents/muse.md")
        .write_str("# Authored agent\n")
        .unwrap();
    dir.child(".mars/skills")
        .write_str("unowned obstruction\n")
        .unwrap();

    sync(dir.path()).arg("--force").assert().failure();
    assert_eq!(
        fs::read_to_string(dir.child(".mars/agents/muse.md").path()).unwrap(),
        "# Selected agent\n",
        "the earlier adoption demonstrates failure happened during apply"
    );
    assert!(
        !dir.child("mars.lock").exists(),
        "failed apply must not publish ownership"
    );

    fs::rename(
        dir.child(".mars/skills").path(),
        dir.child("preserved-obstruction").path(),
    )
    .unwrap();
    sync(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("mars sync --force"));
    assert!(!dir.child("mars.lock").exists());
    sync(dir.path()).arg("--force").assert().success();
    let lock = mars_agents::lock::load(dir.path()).unwrap();
    assert!(lock.contains_output(".mars", "agents/muse.md"));
    assert!(lock.contains_output(".mars", "skills/craft"));
}

fn sync(root: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = offline_mars(root);
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
