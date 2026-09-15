mod common;

use assert_fs::{TempDir, prelude::*};
use common::*;
use mars_agents::lock;
use std::{fs, path::Path};

fn sync(root: &Path) -> assert_cmd::Command {
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

#[test]
fn retry_recovers_canonical_writes_after_a_later_apply_error() {
    for established in [false, true] {
        let dir = TempDir::new().unwrap();
        dir.child("mars.toml").write_str(
            "[package]\nname='demo'\nversion='1.0.0'\n[settings]\ntargets=['.codex']\nagent_emission='never'\n",
        ).unwrap();
        if established {
            dir.child("agents/existing.md")
                .write_str("# Existing\n")
                .unwrap();
            sync(dir.path()).assert().success();
        }
        let old_lock = fs::read(dir.path().join("mars.lock")).ok();
        dir.child("agents/muse.md").write_str("# Muse\n").unwrap();
        dir.child("skills/craft/SKILL.md")
            .write_str("# Craft\n")
            .unwrap();
        dir.child("skills/craft/resources/example.txt")
            .write_str("resource\n")
            .unwrap();
        dir.child(".mars/skills")
            .write_str("unowned obstruction\n")
            .unwrap();

        sync(dir.path()).assert().failure();
        assert_eq!(fs::read(dir.path().join("mars.lock")).ok(), old_lock);
        assert_eq!(
            fs::read_to_string(dir.path().join(".mars/agents/muse.md")).unwrap(),
            "# Muse\n"
        );
        fs::rename(
            dir.path().join(".mars/skills"),
            dir.path().join("preserved-obstruction"),
        )
        .unwrap();

        // No force, lock edits, or manual relocation of the completed output.
        sync(dir.path()).assert().success();
        let recovered = lock::load(dir.path()).unwrap();
        assert!(recovered.contains_output(".mars", "agents/muse.md"));
        assert!(recovered.contains_output(".mars", "skills/craft"));
        assert!(recovered.contains_output(".codex", "skills/craft"));
        if established {
            assert!(recovered.contains_output(".mars", "agents/existing.md"));
        }
        assert_eq!(
            fs::read_to_string(dir.path().join(".codex/skills/craft/resources/example.txt"))
                .unwrap(),
            "resource\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("preserved-obstruction")).unwrap(),
            "unowned obstruction\n"
        );
        let lock_path = dir.path().join("mars.lock");
        let bytes = fs::read(&lock_path).unwrap();
        let modified = fs::metadata(&lock_path).unwrap().modified().unwrap();
        sync(dir.path()).assert().success();
        assert_eq!(fs::read(&lock_path).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&lock_path).unwrap().modified().unwrap(),
            modified
        );
    }
}

fn interrupted_package() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml").write_str(
        "[package]\nname='demo'\nversion='1.0.0'\n[settings]\ntargets=[]\nagent_emission='never'\n",
    ).unwrap();
    dir.child("agents/muse.md").write_str("# Muse\n").unwrap();
    dir.child("skills/craft/SKILL.md")
        .write_str("# Craft\n")
        .unwrap();
    dir.child(".mars/skills")
        .write_str("obstruction\n")
        .unwrap();
    sync(dir.path()).assert().failure();
    assert!(dir.path().join(".mars/agents/muse.md").is_file());
    dir
}

#[test]
fn recovery_survives_multiple_failures_and_source_changes() {
    let dir = interrupted_package();
    dir.child("agents/new.md").write_str("# New\n").unwrap();
    sync(dir.path()).assert().failure();
    assert!(dir.path().join(".mars/agents/new.md").is_file());
    dir.child("agents/muse.md")
        .write_str("# Revised\n")
        .unwrap();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(
        fs::read_to_string(dir.path().join(".mars/agents/muse.md")).unwrap(),
        "# Revised\n"
    );
    let lock = lock::load(dir.path()).unwrap();
    assert!(lock.contains_output(".mars", "agents/new.md"));
    assert!(lock.contains_output(".mars", "agents/muse.md"));
}

#[test]
fn interrupted_output_changed_by_user_is_not_adopted_even_with_force() {
    let dir = interrupted_package();
    dir.child(".mars/agents/muse.md")
        .write_str("# User edits\n")
        .unwrap();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    for force in [false, true] {
        let mut command = sync(dir.path());
        if force {
            command.arg("--force");
        }
        command.assert().failure();
        assert_eq!(
            fs::read_to_string(dir.path().join(".mars/agents/muse.md")).unwrap(),
            "# User edits\n"
        );
        assert!(!dir.path().join("mars.lock").exists());
    }
    fs::rename(
        dir.path().join(".mars/agents/muse.md"),
        dir.path().join("preserved-muse.md"),
    )
    .unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(
        fs::read_to_string(dir.path().join("preserved-muse.md")).unwrap(),
        "# User edits\n"
    );
}

#[test]
fn recovered_output_is_pruned_when_source_was_removed() {
    let dir = interrupted_package();
    fs::remove_file(dir.path().join("agents/muse.md")).unwrap();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    sync(dir.path()).assert().success();
    assert!(!dir.path().join(".mars/agents/muse.md").exists());
    assert!(
        !lock::load(dir.path())
            .unwrap()
            .contains_output(".mars", "agents/muse.md")
    );
}

#[test]
fn recovery_dry_run_does_not_publish_ownership() {
    let dir = interrupted_package();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    let path = dir.path().join(".mars/agents/muse.md");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    sync(dir.path()).arg("--diff").assert().success();
    assert!(!dir.path().join("mars.lock").exists());
    assert!(!dir.path().join(".mars/skills/craft").exists());
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    sync(dir.path()).assert().success();
}

const INTENT: &str = ".mars/pending-canonical.json";

#[test]
fn pending_intent_is_preserved_on_dry_run_and_removed_only_after_commit() {
    let dir = interrupted_package();
    let path = dir.path().join(INTENT);
    let intent = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    sync(dir.path()).arg("--diff").assert().success();
    assert_eq!(fs::read(&path).unwrap(), intent);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    sync(dir.path()).assert().success();
    assert!(!path.exists());

    // Simulate death after final lock rename but before journal removal.
    fs::write(&path, intent).unwrap();
    let old_lock = fs::read(dir.path().join("mars.lock")).unwrap();
    sync(dir.path()).assert().success();
    assert!(!path.exists());
    assert_eq!(fs::read(dir.path().join("mars.lock")).unwrap(), old_lock);
}

#[test]
fn stale_or_corrupt_intent_does_not_claim_unowned_outputs() {
    for corruption in [false, true] {
        let dir = interrupted_package();
        fs::remove_file(dir.path().join(".mars/skills")).unwrap();
        if corruption {
            dir.child(INTENT).write_str("{broken").unwrap();
        } else {
            // Replacing the ownership registry invalidates the old intent.
            lock::write(dir.path(), &lock::LockFile::empty()).unwrap();
        }
        let lock_before = fs::read(dir.path().join("mars.lock")).ok();
        let intent_before = fs::read(dir.path().join(INTENT)).unwrap();
        sync(dir.path()).assert().failure();
        mars()
            .args(["repair", "--root", dir.path().to_str().unwrap()])
            .assert()
            .failure();
        assert_eq!(fs::read(dir.path().join("mars.lock")).ok(), lock_before);
        assert_eq!(fs::read(dir.path().join(INTENT)).unwrap(), intent_before);
        assert_eq!(
            fs::read_to_string(dir.path().join(".mars/agents/muse.md")).unwrap(),
            "# Muse\n"
        );
    }
}

#[test]
fn resolution_failure_does_not_publish_recovered_ownership() {
    let dir = interrupted_package();
    let intent = fs::read(dir.path().join(INTENT)).unwrap();
    dir.child("mars.toml")
        .write_str(
            "[package]\nname='demo'\nversion='1.0.0'\n[dependencies.bad]\npath='./missing'\n",
        )
        .unwrap();
    sync(dir.path()).assert().failure();
    assert!(!dir.path().join("mars.lock").exists());
    assert_eq!(fs::read(dir.path().join(INTENT)).unwrap(), intent);
}

#[cfg(unix)]
#[test]
fn pending_output_and_parent_symlinks_never_authorize_referent_mutation() {
    for parent in [false, true] {
        let dir = interrupted_package();
        fs::remove_file(dir.path().join(".mars/skills")).unwrap();
        let referent = dir.path().join("authored-copy");
        fs::create_dir(&referent).unwrap();
        fs::write(referent.join("muse.md"), "# Muse\n").unwrap();
        fs::remove_file(dir.path().join(".mars/agents/muse.md")).unwrap();
        if parent {
            fs::remove_dir(dir.path().join(".mars/agents")).unwrap();
            std::os::unix::fs::symlink(&referent, dir.path().join(".mars/agents")).unwrap();
        } else {
            std::os::unix::fs::symlink(
                referent.join("muse.md"),
                dir.path().join(".mars/agents/muse.md"),
            )
            .unwrap();
        }
        sync(dir.path()).arg("--force").assert().failure();
        assert!(!dir.path().join("mars.lock").exists());
        assert_eq!(
            fs::read_to_string(referent.join("muse.md")).unwrap(),
            "# Muse\n"
        );
    }
}

#[test]
fn failed_repair_keeps_corrupt_lock_evidence_across_recovery_attempts() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml").write_str(
        "[package]\nname='demo'\nversion='1.0.0'\n[settings]\ntargets=[]\nagent_emission='never'\n",
    ).unwrap();
    dir.child("mars.lock")
        .write_str("preserve this corrupt lock\n")
        .unwrap();
    dir.child("agents/muse.md").write_str("# Muse\n").unwrap();
    dir.child("skills/craft/SKILL.md")
        .write_str("# Craft\n")
        .unwrap();
    dir.child(".mars/skills")
        .write_str("obstruction\n")
        .unwrap();
    for text in ["# Muse\n", "# Revised\n", "# Revised again\n"] {
        dir.child("agents/muse.md").write_str(text).unwrap();
        mars()
            .args(["repair", "--root", dir.path().to_str().unwrap()])
            .env("MARS_OFFLINE", "1")
            .assert()
            .failure();
        assert_eq!(
            fs::read_to_string(dir.path().join("mars.lock")).unwrap(),
            "preserve this corrupt lock\n"
        );
    }
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    mars()
        .args(["repair", "--root", dir.path().to_str().unwrap()])
        .env("MARS_OFFLINE", "1")
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(dir.path().join(".mars/agents/muse.md")).unwrap(),
        "# Revised again\n"
    );
    assert!(
        lock::load(dir.path())
            .unwrap()
            .contains_output(".mars", "agents/muse.md")
    );
}

#[test]
fn frozen_sync_does_not_publish_pending_ownership_even_when_bytes_are_complete() {
    let dir = interrupted_package();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    fs::remove_dir_all(dir.path().join("skills")).unwrap();
    let intent = fs::read(dir.path().join(INTENT)).unwrap();
    sync(dir.path()).arg("--frozen").assert().failure();
    assert!(!dir.path().join("mars.lock").exists());
    assert_eq!(fs::read(dir.path().join(INTENT)).unwrap(), intent);
    sync(dir.path()).assert().success();
}

#[test]
fn malformed_intent_identity_cannot_replace_another_items_ownership() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml").write_str("[package]\nname='demo'\nversion='1.0.0'\n[settings]\ntargets=[]\nagent_emission='never'\n").unwrap();
    dir.child("agents/existing.md")
        .write_str("# Existing\n")
        .unwrap();
    sync(dir.path()).assert().success();
    let old_lock = fs::read(dir.path().join("mars.lock")).unwrap();
    dir.child("agents/muse.md").write_str("# Muse\n").unwrap();
    dir.child("skills/craft/SKILL.md")
        .write_str("# Craft\n")
        .unwrap();
    dir.child(".mars/skills")
        .write_str("obstruction\n")
        .unwrap();
    sync(dir.path()).assert().failure();
    let mut journal: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join(INTENT)).unwrap()).unwrap();
    let items = journal["items"].as_object_mut().unwrap();
    let muse = items.remove("agent/muse").unwrap();
    items.insert("agent/existing".into(), muse);
    let journal = serde_json::to_vec(&journal).unwrap();
    fs::write(dir.path().join(INTENT), &journal).unwrap();
    fs::remove_file(dir.path().join("agents/existing.md")).unwrap();
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    sync(dir.path()).assert().failure();
    assert_eq!(fs::read(dir.path().join("mars.lock")).unwrap(), old_lock);
    assert_eq!(fs::read(dir.path().join(INTENT)).unwrap(), journal);
    assert_eq!(
        fs::read_to_string(dir.path().join(".mars/agents/existing.md")).unwrap(),
        "# Existing\n"
    );
}

#[test]
fn dependency_rename_to_custom_canonical_path_survives_interrupted_apply() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml").write_str("[settings]\ntargets=[]\nagent_emission='never'\n[dependencies.dep]\npath='./source'\nrename={muse='custom/muse.md'}\n").unwrap();
    dir.child("source/mars.toml")
        .write_str("[package]\nname='dep'\nversion='1.0.0'\n")
        .unwrap();
    dir.child("source/agents/muse.md")
        .write_str("# Muse\n")
        .unwrap();
    dir.child(".mars-src/skills/craft/SKILL.md")
        .write_str("# Craft\n")
        .unwrap();
    dir.child(".mars/skills")
        .write_str("obstruction\n")
        .unwrap();
    sync(dir.path()).assert().failure();
    assert!(dir.path().join(".mars/custom/muse.md").is_file());
    fs::remove_file(dir.path().join(".mars/skills")).unwrap();
    sync(dir.path()).assert().success();
    assert!(
        lock::load(dir.path())
            .unwrap()
            .contains_output(".mars", "custom/muse.md")
    );
}
