mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use std::fs;
use std::path::Path;

use common::mars;

/// A host bundle and a package that both provide the hook `guard`, the MCP server `meridian` and
/// the skill `probe__meridian`.
fn write_bundle(root: &Path, with_package_manifest: bool) {
    let write = |path: &str, content: &str| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    };
    if with_package_manifest {
        write(
            "mars.toml",
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nschema = 1\n",
        );
    }
    write(
        "hooks/guard/hook.toml",
        "visibility = \"exported\"\n[targets.\".agy\"]\nnative-hook = \"agy.json\"\n",
    );
    write("hooks/guard/agy.json", "{\"PreToolUse\":[]}");
    write("hooks/guard/run.sh", "echo guard\n");
    write(
        "mcp/meridian/mcp.toml",
        "command = \"node\"\nargs = [\"server.js\"]\nvisibility = \"exported\"\n",
    );
    write(
        "skills/probe__meridian/SKILL.md",
        "---\nname: probe__meridian\ndescription: Probe skill\n---\nHello.\n",
    );
}

fn project(dir: &TempDir) -> std::path::PathBuf {
    let root = dir.child("project");
    root.create_dir_all().unwrap();
    root.child("mars.toml")
        .write_str("[settings]\ntargets = [\".claude\", \".agy\"]\n")
        .unwrap();
    root.path().to_path_buf()
}

fn provide(root: &Path, bundle: &Path) {
    mars()
        .args([
            "provide",
            &format!("meridian@0.1={}", bundle.display()),
            "--root",
        ])
        .arg(root)
        .assert()
        .success();
}

fn sync(root: &Path) -> String {
    let output = mars().args(["sync", "--root"]).arg(root).assert().success();
    String::from_utf8(output.get_output().stderr.clone()).unwrap()
        + &String::from_utf8(output.get_output().stdout.clone()).unwrap()
}

fn add_package(root: &Path, package: &Path) -> String {
    let output = mars()
        .args(["add"])
        .arg(package)
        .arg("--root")
        .arg(root)
        .assert()
        .success();
    String::from_utf8(output.get_output().stderr.clone()).unwrap()
        + &String::from_utf8(output.get_output().stdout.clone()).unwrap()
}

#[test]
fn a_hosts_content_is_recorded_beside_mars_lock_not_in_it() {
    let dir = TempDir::new().unwrap();
    let root = project(&dir);
    let bundle = dir.child("bundle");
    write_bundle(bundle.path(), false);
    provide(&root, bundle.path());
    sync(&root);

    // Installed into the target that takes provided content, and into none that is launched.
    assert!(root.join(".agy/skills/probe__meridian/SKILL.md").is_file());
    assert!(root.join(".agy/hooks/guard").is_dir());
    assert!(!root.join(".claude/skills/probe__meridian").exists());

    let committed = fs::read_to_string(root.join("mars.lock")).unwrap();
    for internal in ["_provided-", "mcp:", "skill/probe__meridian", "hook/guard"] {
        assert!(
            !committed.contains(internal),
            "mars.lock holds `{internal}`:\n{committed}"
        );
    }
    let local = fs::read_to_string(root.join(".mars/machine.lock")).unwrap();
    // The skill has target outputs only: its item is built from the sync's outputs.
    assert!(
        local.contains("[items.\"skill/probe__meridian\"]")
            && local.contains("source = \"_provided-meridian\"")
            && local.contains("target_root = \".agy\""),
        "{local}"
    );
    assert!(local.contains("mcp:meridian"), "{local}");
}

#[test]
fn a_hosts_hook_and_mcp_server_yield_to_a_package_with_one_warning_in_either_order() {
    let dir = TempDir::new().unwrap();
    let package = dir.child("pkg");
    write_bundle(package.path(), true);
    let bundle = dir.child("bundle");
    write_bundle(bundle.path(), false);
    let yielded = |text: &str| text.matches("from meridian is not installed").count();

    // Package first, then the host arrives.
    let first = project(&dir);
    add_package(&first, package.path());
    provide(&first, bundle.path());
    let arrival = sync(&first);
    assert_eq!(
        yielded(&arrival),
        2,
        "hook and MCP server, once each:\n{arrival}"
    );
    assert_eq!(yielded(&sync(&first)), 0);

    // Host first, then the package arrives.
    fs::remove_dir_all(&first).unwrap();
    let second = project(&dir);
    provide(&second, bundle.path());
    sync(&second);
    let arrival = add_package(&second, package.path());
    assert!(
        arrival.contains("hook guard@agy from meridian is not installed"),
        "{arrival}"
    );
    assert_eq!(yielded(&sync(&second)), 0);

    // The package's copy is the installed one, and nothing of the host's is named by its
    // internal source name in what the user read.
    assert!(!arrival.contains("_provided-"), "{arrival}");
    let committed = fs::read_to_string(second.join("mars.lock")).unwrap();
    assert!(committed.contains("hook/guard@agy") || committed.contains("hook/guard"));
}
