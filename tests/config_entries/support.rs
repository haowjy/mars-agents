pub use assert_fs::TempDir;
pub use assert_fs::prelude::*;
pub use predicates::prelude::*;
pub use std::fs;
pub use toml::Value;

pub use crate::common::*;

pub fn contains_path(path: &str) -> impl Predicate<str> + '_ {
    predicate::function(move |value: &str| value.replace('\\', "/").contains(path))
}

pub fn write_hook(project: &assert_fs::fixture::ChildPath, dir_name: &str, manifest: &str) {
    let hook = project.child("hooks").child(dir_name);
    hook.create_dir_all().unwrap();
    hook.child("hook.toml").write_str(manifest).unwrap();
    hook.child("run.sh").write_str("#!/bin/sh\n").unwrap();
}

pub fn write_fragment(
    project: &assert_fs::fixture::ChildPath,
    dir_name: &str,
    file: &str,
    json: &str,
) {
    project
        .child("hooks")
        .child(dir_name)
        .child(file)
        .write_str(json)
        .unwrap();
}

pub fn sync(project: &assert_fs::fixture::ChildPath) -> assert_cmd::assert::Assert {
    mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert()
}

pub fn sync_force(project: &assert_fs::fixture::ChildPath) -> assert_cmd::assert::Assert {
    mars()
        .args([
            "sync",
            "--force",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert()
}

pub fn file_fragment_targets() -> [(&'static str, &'static str); 2] {
    [
        (".opencode", "plugins/mars-audit.ts"),
        (".pi", "extensions/mars-audit.ts"),
    ]
}

pub fn configure_file_fragment(project: &assert_fs::fixture::ChildPath, target: &str) {
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
        .unwrap();
    write_hook(
        project,
        "audit",
        &format!("[targets.\"{target}\"]\nfragment = \"plugin.ts\"\n"),
    );
    write_fragment(
        project,
        "audit",
        "plugin.ts",
        "const SCRIPT = \"${MARS_HOOK_DIR}/run.sh\"\n",
    );
}

pub fn write_dependency_hook(
    source: &assert_fs::fixture::ChildPath,
    name: &str,
    target: &str,
    event: &str,
    command: &str,
) {
    write_hook(
        source,
        name,
        &format!("visibility = \"exported\"\n[targets.\"{target}\"]\n"),
    );
    write_fragment(
        source,
        name,
        &format!("{}.json", target.trim_start_matches('.')),
        &format!(r#"{{"{event}":[{{"hooks":[{{"type":"command","command":"{command}"}}]}}]}}"#),
    );
}

pub fn assert_hook_target_owner(
    project: &assert_fs::fixture::ChildPath,
    canonical_path: &str,
    target_root: &str,
) {
    let lock: Value =
        toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap()).unwrap();
    let owner = lock["items"]
        .as_table()
        .unwrap()
        .values()
        .find(|item| {
            item["outputs"].as_array().is_some_and(|outputs| {
                outputs.iter().any(|output| {
                    output["target_root"].as_str() == Some(".mars")
                        && output["dest_path"].as_str() == Some(canonical_path)
                })
            })
        })
        .expect("canonical hook owner missing");
    assert!(
        owner["outputs"].as_array().unwrap().iter().any(|output| {
            output["target_root"].as_str() == Some(target_root)
                && output["dest_path"].as_str() == Some("hooks/audit")
        }),
        "{target_root} output was attached to the wrong scoped hook owner"
    );
}
