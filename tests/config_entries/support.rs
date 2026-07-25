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
    let assertion = mars()
        .args(["sync", "--root", project.path().to_str().unwrap()])
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert_config_entry_consistency_with_diagnostics(project, &stderr);
    assertion
}

pub fn sync_force(project: &assert_fs::fixture::ChildPath) -> assert_cmd::assert::Assert {
    let assertion = mars()
        .args([
            "sync",
            "--force",
            "--root",
            project.path().to_str().unwrap(),
        ])
        .assert();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert_config_entry_consistency_with_diagnostics(project, &stderr);
    assertion
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

/// Cross-artifact oracle for the config-entry lane.
///
/// A sync may leave a malformed user-owned config file untouched, so unreadable
/// files are skipped. Managed file-hook records have three disk states: a regular
/// file must match its checksum unless that exact output was reported as
/// user-edited; a non-file needs no checksum check because retaining its record
/// preserves authority to retry deletion; an absent path must not retain a record,
/// because that would be ghost ownership.
pub fn assert_config_entry_consistency(project: &assert_fs::fixture::ChildPath) {
    assert_config_entry_consistency_with_diagnostics(project, "");
}

fn assert_config_entry_consistency_with_diagnostics(
    project: &assert_fs::fixture::ChildPath,
    diagnostics: &str,
) {
    let lock_path = project.child("mars.lock");
    if !lock_path.exists() {
        return;
    }
    let lock: mars_agents::lock::LockFile =
        toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();

    for (target, records) in &lock.config_entries {
        for (key, record) in records {
            if let Some(emitted) = &record.emitted_json {
                let event = key
                    .strip_prefix("hook:")
                    .and_then(|rest| rest.split_once(':'))
                    .map(|(event, _)| event)
                    .expect("emitted config-entry record must be a hook");
                let file = match target.as_str() {
                    ".claude" => "settings.local.json",
                    ".codex" | ".cursor" => "hooks.json",
                    other => panic!("{other} cannot own merge-mode hook records"),
                };
                let path = project.child(target).child(file);
                let Ok(raw) = fs::read_to_string(path.path()) else {
                    panic!(
                        "lock owns `{key}` but `{}` is absent",
                        path.path().display()
                    );
                };
                let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                let expected: Vec<serde_json::Value> = serde_json::from_str(emitted).unwrap();
                let actual = root["hooks"][event].as_array().unwrap_or_else(|| {
                    panic!("lock owns `{key}` but hooks.{event} is absent on disk")
                });
                assert!(
                    expected.iter().all(|entry| actual.contains(entry)),
                    "lock owns `{key}` but its emitted entries do not exist on disk"
                );
            } else if let Some(name) = key.strip_prefix("mcp:") {
                let file = match target.as_str() {
                    ".claude" => ".mcp.json",
                    ".codex" => "codex_mcp.json",
                    ".cursor" => "mcp.json",
                    ".opencode" => "opencode.json",
                    _ => continue,
                };
                let path = project.child(target).child(file);
                let Ok(raw) = fs::read_to_string(path.path()) else {
                    panic!(
                        "lock owns `{key}` but `{}` is absent",
                        path.path().display()
                    );
                };
                let Ok(root) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                assert!(
                    root["mcpServers"].get(name).is_some(),
                    "lock owns `{key}` but mcpServers.{name} is absent on disk"
                );
            }
            // A hook record without emitted_json is the explicit one-release
            // legacy migration state and cannot be content-matched.
        }
    }

    for item in lock
        .items
        .values()
        .filter(|item| item.kind == mars_agents::lock::ItemKind::Hook)
    {
        for output in &item.outputs {
            if output.target_root == ".mars"
                || !(output.dest_path.as_str().starts_with("plugins/mars-")
                    || output.dest_path.as_str().starts_with("extensions/mars-"))
            {
                continue;
            }
            let path = project
                .child(&output.target_root)
                .child(output.dest_path.as_str());
            let output_divergence_was_reported = diagnostics.contains(&format!(
                "target `{}` item `{}` was edited after Mars installed it",
                output.target_root, output.dest_path
            ));
            let metadata = match fs::symlink_metadata(path.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    panic!(
                        "lock owns file hook `{}/{}` but it is absent",
                        output.target_root, output.dest_path
                    );
                }
                Err(error) => panic!(
                    "failed to inspect owned file hook `{}/{}`: {error}",
                    output.target_root, output.dest_path
                ),
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            if output_divergence_was_reported {
                continue;
            }
            let bytes = fs::read(path.path()).unwrap_or_else(|error| {
                panic!(
                    "failed to read owned file hook `{}/{}`: {error}",
                    output.target_root, output.dest_path
                )
            });
            assert_eq!(
                mars_agents::hash::hash_bytes(&bytes),
                output.installed_checksum.as_ref(),
                "file-hook bytes disagree with lock checksum for `{}/{}`",
                output.target_root,
                output.dest_path
            );
        }
    }
}
