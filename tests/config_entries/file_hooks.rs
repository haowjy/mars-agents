use super::support::*;

#[test]
fn opencode_file_hook_ignores_malformed_unrelated_config() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    configure_file_fragment(&project, ".opencode");
    project
        .child(".opencode/opencode.json")
        .write_str("{ malformed")
        .unwrap();

    sync(&project).success();

    project
        .child(".opencode/plugins/mars-audit.ts")
        .assert(predicate::str::contains("hooks/audit/run.sh"));
    project
        .child(".opencode/opencode.json")
        .assert("{ malformed");
}

#[test]
fn legacy_opencode_hook_records_reach_the_removal_only_sweep() {
    let dir = TempDir::new().unwrap();
    let project = dir.child("project");
    project.create_dir_all().unwrap();
    project
        .child("mars.toml")
        .write_str("[settings]\ntargets = [\".opencode\"]\n")
        .unwrap();
    write_hook(
        &project,
        "audit",
        "[targets.\".opencode\"]\nfragment = \"plugin.ts\"\n",
    );
    write_fragment(&project, "audit", "plugin.ts", "export default {}");
    sync(&project).success();

    let lock_path = project.child("mars.lock");
    let mut lock: Value = toml::from_str(&fs::read_to_string(lock_path.path()).unwrap()).unwrap();
    let mut records = toml::Table::new();
    records.insert(
        "hook:tool.pre:audit".into(),
        toml::Table::from_iter([("source".into(), Value::String("_self".into()))]).into(),
    );
    lock.as_table_mut().unwrap().insert(
        "config_entries".into(),
        Value::Table(toml::Table::from_iter([(
            ".opencode".into(),
            Value::Table(records),
        )])),
    );
    lock_path
        .write_str(&toml::to_string(&lock).unwrap())
        .unwrap();
    project
        .child(".opencode/opencode.json")
        .write_str(
            r#"{"user":true,"hooks":{"tool.pre":["/cache/hooks/audit/run.sh","printf user"]}}"#,
        )
        .unwrap();
    fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();

    sync(&project).success();
    let json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.child(".opencode/opencode.json").path()).unwrap(),
    )
    .unwrap();
    assert_eq!(json["user"], true);
    assert_eq!(
        json["hooks"]["tool.pre"],
        serde_json::json!(["printf user"])
    );
}

#[test]
fn file_fragments_place_substitute_remove_and_resync_idempotently() {
    for (target, destination, export) in [
        (
            ".opencode",
            "plugins/mars-audit.ts",
            r#"import type { Plugin } from "@opencode-ai/plugin"
const SCRIPT = "${MARS_HOOK_DIR}/run.sh"
export const Audit: Plugin = async ({ $ }) => ({
  "tool.execute.before": async () => { await $`bash ${SCRIPT}` }
})"#,
        ),
        (
            ".pi",
            "extensions/mars-audit.ts",
            r#"import type { ExtensionAPI } from "@mariozechner/pi-coding-agent"
const SCRIPT = "${MARS_HOOK_DIR}/run.sh"
export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async () => { await pi.exec("bash", [SCRIPT]) })
}"#,
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str(&format!("[settings]\ntargets = [\"{target}\"]\n"))
            .unwrap();
        write_hook(
            &project,
            "audit",
            &format!("[targets.\"{target}\"]\nfragment = \"plugin.ts\"\n"),
        );
        write_fragment(&project, "audit", "plugin.ts", export);
        sync(&project).success();
        let placed = project.child(target).child(destination);
        let first = fs::read_to_string(placed.path()).unwrap();
        assert!(!first.contains("${MARS_HOOK_DIR}"));
        assert!(first.contains(&format!("{target}/hooks/audit/run.sh")));
        let first_lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
        sync(&project).success();
        assert_eq!(fs::read_to_string(placed.path()).unwrap(), first);
        assert_eq!(
            fs::read_to_string(project.child("mars.lock").path()).unwrap(),
            first_lock
        );
        project
            .child(target)
            .child(if target == ".opencode" {
                "plugins/user.ts"
            } else {
                "extensions/user.ts"
            })
            .write_str("user")
            .unwrap();
        fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
        sync(&project).success();
        placed.assert(predicate::path::missing());
        project
            .child(target)
            .child(if target == ".opencode" {
                "plugins/user.ts"
            } else {
                "extensions/user.ts"
            })
            .assert("user");
    }
}

#[test]
fn untracked_file_fragment_collision_and_later_removal_preserve_user_file() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);
        let placed = project.child(target).child(destination);
        placed.write_str("user content").unwrap();

        sync(&project)
            .success()
            .stderr(predicate::str::contains("preserved local content"));
        placed.assert("user content");
        fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
        sync(&project).success();
        placed.assert("user content");
    }
}

#[test]
fn force_adopts_file_fragment_and_records_exact_output() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);
        project
            .child(target)
            .child(destination)
            .write_str("user content")
            .unwrap();

        sync_force(&project)
            .success()
            .stderr(predicate::str::contains("adopting with `--force`"));
        let lock: mars_agents::lock::LockFile =
            toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap())
                .unwrap();
        let output = lock
            .items
            .values()
            .flat_map(|item| &item.outputs)
            .find(|output| output.target_root == target && output.dest_path.as_str() == destination)
            .expect("forced file fragment must have an exact output record");
        let actual = mars_agents::hash::hash_bytes(
            &fs::read(project.child(target).child(destination).path()).unwrap(),
        );
        assert_eq!(
            output
                .installed_checksum()
                .expect("installed output")
                .as_ref(),
            actual
        );
        assert!(
            !fs::read_to_string(project.child("mars.lock").path())
                .unwrap()
                .contains("hook-file:")
        );
    }
}

#[test]
fn force_does_not_emit_file_fragment_without_canonical_owner() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);

        let canonical = project
            .child(".mars/hooks")
            .child(target.trim_start_matches('.'))
            .child("audit");
        canonical.create_dir_all().unwrap();
        canonical.child("unmanaged").write_str("user").unwrap();

        let installed = project.child(target).child("hooks/audit");
        installed.create_dir_all().unwrap();
        installed
            .child("hook.toml")
            .write_str(&format!(
                "[targets.\"{target}\"]\nfragment = \"plugin.ts\"\n"
            ))
            .unwrap();
        installed.child("run.sh").write_str("#!/bin/sh\n").unwrap();
        installed
            .child("plugin.ts")
            .write_str("const SCRIPT = \"${MARS_HOOK_DIR}/run.sh\"\n")
            .unwrap();

        sync_force(&project)
            .success()
            .stderr(predicate::str::contains("unmanaged"));

        project
            .child(target)
            .child(destination)
            .assert(predicate::path::missing());
        let lock: mars_agents::lock::LockFile =
            toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap())
                .unwrap();
        assert!(
            !lock
                .items
                .values()
                .flat_map(|item| &item.outputs)
                .any(|output| output.target_root == target
                    && output.dest_path.as_str() == destination),
            "an emitted file fragment must always have a canonical owner"
        );
    }
}

#[test]
fn hand_edited_managed_file_fragment_is_not_silently_replaced() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);
        sync(&project).success();
        let placed = project.child(target).child(destination);
        placed.write_str("hand edited").unwrap();

        sync(&project).success();
        placed.assert("hand edited");
    }
}

#[test]
fn failed_file_fragment_removal_retains_ownership_for_retry() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);
        sync(&project).success();

        let placed = project.child(target).child(destination);
        fs::remove_file(placed.path()).unwrap();
        placed.create_dir_all().unwrap();
        fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();

        sync(&project)
            .success()
            .stderr(predicate::str::contains("failed to remove"));
        placed.assert(predicate::path::is_dir());
        let lock: mars_agents::lock::LockFile =
            toml::from_str(&fs::read_to_string(project.child("mars.lock").path()).unwrap())
                .unwrap();
        assert!(
            !lock.contains_output(
                ".mars",
                &format!("hooks/{}/audit", target.trim_start_matches('.'))
            ),
            "retry tombstone must not resurrect canonical ownership"
        );
        let retained = lock
            .items
            .values()
            .flat_map(|item| &item.outputs)
            .find(|output| output.target_root == target && output.dest_path.as_str() == destination)
            .expect("failed removal must retain ownership authority");
        assert!(matches!(
            retained.state,
            mars_agents::lock::OutputState::PendingDeletion
        ));

        let canonical = project
            .child(".mars/hooks")
            .child(target.trim_start_matches('.'))
            .child("audit");
        canonical.create_dir_all().unwrap();
        canonical.child("user.txt").write_str("unmanaged").unwrap();
        sync(&project).success();
        canonical
            .child("user.txt")
            .assert(predicate::str::contains("unmanaged"));
        placed.assert(predicate::path::missing());
        let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
        assert!(!lock.contains(destination));
    }
}

#[test]
fn removing_file_fragment_deletes_only_lock_owned_destination() {
    for (target, destination) in file_fragment_targets() {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        configure_file_fragment(&project, target);
        sync(&project).success();
        let placed = project.child(target).child(destination);
        placed.assert(predicate::path::is_file());
        placed.write_str("edited but still lock owned").unwrap();

        fs::remove_dir_all(project.child("hooks/audit").path()).unwrap();
        sync(&project).success();
        placed.assert(predicate::path::missing());
        let lock = fs::read_to_string(project.child("mars.lock").path()).unwrap();
        assert!(!lock.contains(destination));
    }
}

#[test]
fn file_fragment_preflight_errors_do_not_mutate_and_unchecked_is_rejected() {
    for manifest in [
        "[targets.\".opencode\"]\nfragment = \"missing.ts\"\n",
        "[targets.\".opencode\"]\nfragment = \"plugin.ts\"\nunchecked = false\n",
    ] {
        let dir = TempDir::new().unwrap();
        let project = dir.child("project");
        project.create_dir_all().unwrap();
        project
            .child("mars.toml")
            .write_str("[settings]\ntargets = [\".opencode\"]\n")
            .unwrap();
        project.child(".opencode/plugins").create_dir_all().unwrap();
        project
            .child(".opencode/plugins/sentinel.ts")
            .write_str("sentinel")
            .unwrap();
        write_hook(&project, "bad", manifest);
        if manifest.contains("plugin.ts") {
            write_fragment(&project, "bad", "plugin.ts", "export default {}");
        }
        let assertion = sync(&project).failure();
        if manifest.contains("missing.ts") {
            assertion.stderr(predicate::str::contains("failed to read fragment"));
        } else {
            assertion.stderr(predicate::str::contains("`unchecked` is not supported"));
        }
        project
            .child(".opencode/plugins/sentinel.ts")
            .assert("sentinel");
        project
            .child(".opencode/plugins/mars-bad.ts")
            .assert(predicate::path::missing());
        assert!(!project.child(".mars/hooks/bad").exists());
    }
}
