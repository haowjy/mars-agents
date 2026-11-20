mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::fs;

use common::*;

type Project = assert_fs::fixture::ChildPath;

fn project(dir: &TempDir, targets: &str) -> Project {
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!("[settings]\ntargets = {targets}\n"))
        .unwrap();
    project
        .child(".mars-src/skills/review/SKILL.md")
        .write_str("---\nname: review\ndescription: d\n---\nBody\n")
        .unwrap();
    project
}

fn mars_in(dir: &TempDir, project: &Project, args: &[&str]) -> assert_cmd::assert::Assert {
    offline_mars(dir.path())
        .args(args)
        .args(["--root", project.path().to_str().unwrap()])
        .assert()
        .success()
}

fn text(project: &Project, file: &str) -> String {
    fs::read_to_string(project.child(file).path()).unwrap_or_default()
}

#[test]
fn link_local_adds_a_target_for_this_machine_and_leaves_the_committed_files_alone() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude']");
    mars_in(&dir, &project, &["sync"]);
    let (toml, lock) = (text(&project, "mars.toml"), text(&project, "mars.lock"));

    mars_in(&dir, &project, &["link", ".agy", "--local"]);
    mars_in(&dir, &project, &["sync"]);

    assert_eq!(text(&project, "mars.toml"), toml);
    assert_eq!(
        text(&project, "mars.lock"),
        lock,
        "no `.agy` record is shared"
    );
    assert!(text(&project, "mars.local.toml").contains(".agy"));
    let machine = text(&project, ".mars/machine.lock");
    assert!(machine.contains(r#"target_root = ".agy""#), "{machine}");
    assert!(
        !machine.contains(r#"target_root = ".mars""#),
        "the canonical copy stays in mars.lock"
    );
    assert!(project.child(".agy/skills/review/SKILL.md").exists());
    assert!(
        project.child(".claude/skills/review/SKILL.md").exists(),
        "the local list adds to the project's"
    );

    // A clone that has only the committed files syncs `.claude` and warns of nothing.
    let other = TempDir::new().unwrap();
    let clone = self::project(&other, "['.claude']");
    mars_in(&other, &clone, &["sync"]);
    fs::write(clone.child("mars.lock").path(), &lock).unwrap();
    mars_in(&other, &clone, &["sync"]).stderr(predicate::str::contains("mars unlink").not());
    assert_eq!(text(&clone, "mars.lock"), lock);
}

#[test]
fn a_target_moves_between_the_files_with_its_records() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude']");
    mars_in(&dir, &project, &["sync"]);
    let shared = text(&project, "mars.lock");
    mars_in(&dir, &project, &["link", ".agy", "--local"]);
    mars_in(&dir, &project, &["sync"]);

    // To the project: `link` takes it out of mars.local.toml, and the records follow.
    mars_in(&dir, &project, &["link", ".agy"]);
    assert!(text(&project, "mars.toml").contains(".agy"));
    assert!(!text(&project, "mars.local.toml").contains(".agy"));
    assert!(!project.child(".mars/machine.lock").exists());
    let project_lock = text(&project, "mars.lock");
    assert!(project_lock.contains(r#"target_root = ".agy""#));

    // Back to this machine, by hand: the next sync moves the records.
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = ['.claude']\n")
        .unwrap();
    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = ['.agy']\n")
        .unwrap();
    mars_in(&dir, &project, &["sync"]).stdout(predicate::str::contains("removed").not());
    assert_eq!(text(&project, "mars.lock"), shared);
    assert!(text(&project, ".mars/machine.lock").contains(r#"target_root = ".agy""#));
    assert!(project.child(".agy/skills/review/SKILL.md").exists());
}

#[test]
fn a_record_both_files_hold_is_collapsed_by_the_next_sync() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude']");
    mars_in(&dir, &project, &["sync"]);
    mars_in(&dir, &project, &["link", ".agy", "--local"]);
    mars_in(&dir, &project, &["sync"]);
    let machine = text(&project, ".mars/machine.lock");

    // A kill between the writes of a move leaves the record in both files.
    mars_in(&dir, &project, &["link", ".agy"]);
    let moved = text(&project, "mars.lock");
    fs::write(project.child(".mars/machine.lock").path(), machine).unwrap();
    mars_in(&dir, &project, &["sync"]).stderr(predicate::str::contains("mars unlink").not());
    assert_eq!(text(&project, "mars.lock"), moved);
    assert!(!project.child(".mars/machine.lock").exists());
}

#[test]
fn a_target_removed_from_mars_local_toml_by_hand_is_dropped_by_the_next_sync() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude']");
    mars_in(&dir, &project, &["sync"]);
    let shared = text(&project, "mars.lock");
    mars_in(&dir, &project, &["link", ".agy", "--local"]);
    mars_in(&dir, &project, &["sync"]);

    fs::remove_file(project.child("mars.local.toml").path()).unwrap();
    mars_in(&dir, &project, &["sync"]);

    assert!(!project.child(".agy/skills").exists());
    assert!(!project.child(".mars/machine.lock").exists());
    assert_eq!(text(&project, "mars.lock"), shared);
}

#[test]
fn unlink_local_leaves_a_project_target_and_unlink_removes_it_from_both_files() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude', '.agy']");
    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = ['.agy']\n")
        .unwrap();
    mars_in(&dir, &project, &["sync"]);
    let lock = text(&project, "mars.lock");
    assert!(
        lock.contains(r#"target_root = ".agy""#),
        "the project names it"
    );

    mars_in(&dir, &project, &["unlink", ".agy", "--local"]);
    assert!(project.child(".agy/skills/review/SKILL.md").exists());
    assert_eq!(text(&project, "mars.lock"), lock);

    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = ['.agy']\n")
        .unwrap();
    mars_in(&dir, &project, &["unlink", ".agy"]);
    assert!(!project.child(".agy/skills").exists());
    assert!(!text(&project, "mars.toml").contains(".agy"));
    assert!(!text(&project, "mars.local.toml").contains(".agy"));
    assert!(!text(&project, "mars.lock").contains(".agy"));
}

#[test]
fn a_local_only_target_recorded_in_mars_lock_by_mars_0_38_moves_to_machine_lock() {
    let dir = TempDir::new().unwrap();
    let project = project(&dir, "['.claude']");
    mars_in(&dir, &project, &["sync"]);
    let shared = text(&project, "mars.lock");
    // Mars 0.38.0 wrote the records of a target only mars.local.toml names into mars.lock.
    mars_in(&dir, &project, &["link", ".agy"]);
    mars_in(&dir, &project, &["sync"]);
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = ['.claude']\n")
        .unwrap();
    project
        .child("mars.local.toml")
        .write_str("[settings]\ntargets = ['.agy']\n")
        .unwrap();

    mars_in(&dir, &project, &["sync"]).stdout(predicate::str::contains("removed").not());

    assert_eq!(text(&project, "mars.lock"), shared);
    assert!(text(&project, ".mars/machine.lock").contains(r#"target_root = ".agy""#));
    assert!(project.child(".agy/skills/review/SKILL.md").exists());
}
