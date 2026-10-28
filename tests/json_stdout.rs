//! Pins the `--json` contract: stdout is exactly one JSON document, and human narration
//! (warnings, status lines) goes to stderr.

mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;

use common::*;

#[test]
fn outdated_json_keeps_warnings_off_stdout() {
    let dir = TempDir::new().unwrap();
    let source = create_source(
        &dir,
        "src",
        &[("reader", "---\nname: reader\n---\n# R")],
        &[],
    );
    let project = dir.child("proj");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[dependencies.dep]\nurl = \"https://example.invalid/dep.git\"\n")
        .unwrap();
    project
        .child("mars.local.toml")
        .write_str(&format!(
            "[overrides.dep]\npath = \"{}\"\n",
            portable_path(&source)
        ))
        .unwrap();

    let output = offline_mars(dir.path())
        .args([
            "outdated",
            "--json",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str::<serde_json::Value>(&stdout).expect("stdout is one JSON document");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("overridden"),
        "warning moved to stderr: {stderr}"
    );
}
