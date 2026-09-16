mod common;

use assert_fs::{TempDir, prelude::*};
use common::*;
use mars_agents::{discover, frontmatter::Frontmatter};
use std::{fs, path::Path};

const PACKAGE: &str = "[package]\nname = 'demo'\nversion = '1.0.0'\n";
const SETTINGS: &str = "[settings]\ntargets = ['.codex']\nagent_emission = 'never'\n";

fn sync(root: &Path) -> assert_cmd::Command {
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

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

fn assert_noop(root: &Path) {
    let lock = read(root, "mars.lock");
    let output = sync(root)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for field in ["installed", "updated", "removed"] {
        assert_eq!(report[field], 0, "{field}: {report}");
    }
    assert_eq!(read(root, "mars.lock"), lock);
}

#[test]
fn declared_package_preserves_native_metadata_resources_and_catalog_across_syncs() {
    for prepopulated in [false, true] {
        let dir = TempDir::new().unwrap();
        dir.child("mars.toml")
            .write_str(&format!("{PACKAGE}{SETTINGS}"))
            .unwrap();
        let agent = "---\nname: muse\ndescription: Muse\nskills: [craft]\ntools: [Read, Write]\n---\n# Muse\n";
        let skill = "---\nname: craft\ndescription: Craft\nuser-invocable: false\nmodel-invocable: false\n---\n# Craft\n";
        dir.child("agents/muse.md").write_str(agent).unwrap();
        dir.child("skills/craft/SKILL.md").write_str(skill).unwrap();
        dir.child("skills/craft/resources/example.txt")
            .write_str("resource")
            .unwrap();
        if prepopulated {
            dir.child(".codex/agents/unrelated.toml")
                .write_str("unowned")
                .unwrap();
        }
        sync(dir.path()).assert().success();
        assert_eq!(read(dir.path(), ".mars/agents/muse.md"), agent);
        assert_eq!(read(dir.path(), ".mars/skills/craft/SKILL.md"), skill);
        assert_eq!(
            read(dir.path(), ".codex/skills/craft/resources/example.txt"),
            "resource"
        );
        offline_mars(dir.path())
            .args([
                "agents",
                "show",
                "muse",
                "--root",
                dir.path().to_str().unwrap(),
            ])
            .assert()
            .success();
        assert_noop(dir.path());
        assert_eq!(read(dir.path(), ".mars/agents/muse.md"), agent);
        assert_eq!(read(dir.path(), ".mars/skills/craft/SKILL.md"), skill);
        if prepopulated {
            assert_eq!(read(dir.path(), ".codex/agents/unrelated.toml"), "unowned");
        }
        // Generated foreign-container signals must not reinterpret package inputs.
        dir.child(".claude/agents/unrelated.md")
            .write_str("# Unrelated")
            .unwrap();
        assert_noop(dir.path());
    }
}

#[test]
fn undeclared_project_ignores_root_sources_but_keeps_local_overrides() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml").write_str(SETTINGS).unwrap();
    dir.child("agents/muse.md").write_str("# Root").unwrap();
    dir.child(".mars-src/agents/local.md")
        .write_str("# Local")
        .unwrap();
    sync(dir.path()).assert().success();
    assert!(!dir.child(".mars/agents/muse.md").exists());
    assert_eq!(read(dir.path(), ".mars/agents/local.md"), "# Local");
}

#[test]
fn flat_package_uses_declared_name_and_filters_before_staging() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str(&format!(
            "{PACKAGE}[settings]\ntargets = ['.native/custom']\nagent_emission = 'never'\n"
        ))
        .unwrap();
    dir.child("SKILL.md")
        .write_str("# Authored flat skill")
        .unwrap();
    dir.child("resources/example.txt")
        .write_str("resource")
        .unwrap();
    dir.child(".git/control").write_str("control").unwrap();
    dir.child(".claude/skills/old/SKILL.md")
        .write_str("# Old generated")
        .unwrap();
    dir.child(".native/custom/unrelated")
        .write_str("user")
        .unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(
        read(dir.path(), ".mars/skills/demo/SKILL.md"),
        "# Authored flat skill"
    );
    assert_eq!(
        read(dir.path(), ".mars/skills/demo/resources/example.txt"),
        "resource"
    );
    for excluded in [
        ".mars",
        ".git",
        "mars.toml",
        "mars.lock",
        ".native/custom",
        ".claude",
    ] {
        assert!(
            !dir.child(format!(".mars/skills/demo/{excluded}")).exists(),
            "copied {excluded}"
        );
    }
    assert!(!dir.child(".mars/skills/_self").exists());
    assert_noop(dir.path());
    dir.child("SKILL.md").write_str("# Authored edit").unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(
        read(dir.path(), ".mars/skills/demo/SKILL.md"),
        "# Authored edit"
    );
    assert_noop(dir.path());
    assert_eq!(read(dir.path(), ".native/custom/unrelated"), "user");
}

#[test]
fn nonhidden_output_promotion_remains_an_explicit_discovery_limitation() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str(&format!(
            "{PACKAGE}[settings]\ntargets = ['out/native']\nagent_emission = 'never'\n"
        ))
        .unwrap();
    dir.child("SKILL.md")
        .write_str("# Root before emission")
        .unwrap();
    let before = discover::discover_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(before[0].source_path, Path::new("."));
    sync(dir.path()).assert().success();
    assert!(!dir.child(".mars/skills/demo/out/native").exists());
    let after = discover::discover_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(after[0].source_path, Path::new("out/native/skills/demo"));
    dir.child("SKILL.md")
        .write_str("# Root edit is now suppressed")
        .unwrap();
    assert_noop(dir.path());
    assert_eq!(
        read(dir.path(), ".mars/skills/demo/SKILL.md"),
        "# Root before emission"
    );
}

#[test]
fn nested_distribution_is_promoted_only_after_shallower_items_disappear() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str(&format!("{PACKAGE}{SETTINGS}"))
        .unwrap();
    dir.child("agents/muse.md").write_str("# Root").unwrap();
    dir.child("cw/agents/muse.md")
        .write_str("# Distribution")
        .unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(read(dir.path(), ".mars/agents/muse.md"), "# Root");
    fs::remove_file(dir.child("agents/muse.md").path()).unwrap();
    sync(dir.path()).assert().success();
    assert_eq!(read(dir.path(), ".mars/agents/muse.md"), "# Distribution");
    dir.child("mars.toml").write_str(SETTINGS).unwrap();
    sync(dir.path()).assert().success();
    assert!(!dir.child(".mars/agents/muse.md").exists());
    assert_eq!(read(dir.path(), "cw/agents/muse.md"), "# Distribution");
}

#[test]
fn self_overlays_installed_names_after_explicit_and_collision_renames() {
    for collision in [false, true] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        let a = create_source(
            &dir,
            "a",
            &[("writer", "# Dependency A")],
            &[("craft", "# Craft A")],
        );
        let b = create_source(
            &dir,
            "b",
            &[("writer", "# Dependency B")],
            &[("craft", "# Craft B")],
        );
        let deps = if collision {
            format!(
                "[dependencies.a]\npath = '{}'\n[dependencies.b]\npath = '{}'\n",
                portable_path(&a),
                portable_path(&b)
            )
        } else {
            format!(
                "[dependencies.a]\npath = '{}'\nrename = {{ writer = 'editor', craft = 'renamed' }}\n",
                portable_path(&a)
            )
        };
        project
            .child("mars.toml")
            .write_str(&format!("{PACKAGE}{SETTINGS}{deps}"))
            .unwrap();
        project
            .child("agents/writer.md")
            .write_str("---\nname: writer\nskills: [craft]\n---\n# Self writer")
            .unwrap();
        sync(project.path()).assert().success();
        let name = if collision { "craft__a" } else { "renamed" };
        let fm = Frontmatter::parse(&read(project.path(), ".mars/agents/writer.md")).unwrap();
        assert_eq!(
            fm.get("skills").unwrap().as_sequence().unwrap()[0].as_str(),
            Some(name)
        );
        let dependency_agent = if collision { "writer__a" } else { "editor" };
        assert_eq!(
            read(
                project.path(),
                &format!(".mars/agents/{dependency_agent}.md")
            ),
            "# Dependency A"
        );
        if collision {
            assert_eq!(
                read(project.path(), ".mars/agents/writer__b.md"),
                "# Dependency B"
            );
        }
        project
            .child("skills/craft/SKILL.md")
            .write_str("# Self craft")
            .unwrap();
        sync(project.path()).assert().success();
        let fm = Frontmatter::parse(&read(project.path(), ".mars/agents/writer.md")).unwrap();
        assert_eq!(
            fm.get("skills").unwrap().as_sequence().unwrap()[0].as_str(),
            Some("craft")
        );
        assert!(
            project
                .child(format!(".mars/skills/{name}/SKILL.md"))
                .exists()
        );
        // A self name matching the *installed* dependency name replaces it.
        project
            .child(format!("agents/{dependency_agent}.md"))
            .write_str("# Self replacement")
            .unwrap();
        sync(project.path()).assert().success();
        assert_eq!(
            read(
                project.path(),
                &format!(".mars/agents/{dependency_agent}.md")
            ),
            "# Self replacement"
        );
        assert_noop(project.path());
    }
}

#[test]
fn flat_package_filters_alternate_spellings_of_configured_output_paths() {
    for spelling in ["absolute", "dot", "parent"] {
        let dir = TempDir::new().unwrap();
        let target = match spelling {
            "absolute" => portable_path(dir.child(".native").path()),
            "dot" => "./.native".to_string(),
            _ => "unused/../.native".to_string(),
        };
        dir.child("mars.toml")
            .write_str(&format!("{PACKAGE}[settings]\ntargets = ['{target}']\n"))
            .unwrap();
        dir.child("SKILL.md").write_str("# Flat source").unwrap();
        sync(dir.path()).assert().success();
        assert_noop(dir.path());
        assert!(
            !dir.child(".mars/skills/demo/.native").exists(),
            "copied target: {spelling}"
        );
        dir.child("SKILL.md").write_str("# Edit").unwrap();
        sync(dir.path()).assert().success();
        assert_eq!(read(dir.path(), ".mars/skills/demo/SKILL.md"), "# Edit");
        assert_noop(dir.path());
    }
}

#[test]
fn flat_package_does_not_import_previously_owned_custom_targets() {
    let dir = TempDir::new().unwrap();
    dir.child("SKILL.md").write_str("# Flat").unwrap();
    dir.child("mars.toml")
        .write_str(&format!("{PACKAGE}[settings]\ntargets = ['.old/native']\n"))
        .unwrap();
    sync(dir.path()).assert().success();
    dir.child("mars.toml")
        .write_str(&format!("{PACKAGE}{SETTINGS}"))
        .unwrap();
    sync(dir.path()).assert().success();
    assert!(!dir.child(".mars/skills/demo/.old/native").exists());
    assert_noop(dir.path());
}

#[test]
fn flat_override_keeps_authored_resources_named_like_project_outputs() {
    let dir = TempDir::new().unwrap();
    dir.child("mars.toml")
        .write_str("[settings]\ntargets = ['.native/custom']\n")
        .unwrap();
    dir.child(".mars-src/SKILL.md")
        .write_str("# Local flat")
        .unwrap();
    for resource in [
        ".native/custom/resource.txt",
        ".codex/resource.txt",
        ".mars-src/resource.txt",
    ] {
        dir.child(format!(".mars-src/{resource}"))
            .write_str("authored resource")
            .unwrap();
    }
    sync(dir.path()).assert().success();
    for resource in [
        ".native/custom/resource.txt",
        ".codex/resource.txt",
        ".mars-src/resource.txt",
    ] {
        assert_eq!(
            read(dir.path(), &format!(".mars/skills/_self/{resource}")),
            "authored resource"
        );
    }
    assert_noop(dir.path());
}
