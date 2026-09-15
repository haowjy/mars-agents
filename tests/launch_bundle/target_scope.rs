//! Configured target permission is independent of executable discovery.
use assert_fs::TempDir;
use serde_json::Value;

use super::common::{replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, install_logging_harnesses, mars_cmd};

fn launch_with_scope(
    settings: &str,
    local: Option<&str>,
    args: &[&str],
) -> (std::process::Output, String) {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_logging_harnesses(temp.path());
    let log = temp.path().join("commands.log");
    let config = format!(
        r#"[settings]
agent_emission = "never"
{settings}
[models.primary]
model = "claude-opus-4-6"
[models.backup]
model = "gpt-5"
"#
    );
    let (server, root) = setup_bundle_project(
        &temp,
        "scope-source",
        "---\nname: reviewer\nharness: claude\nmodel: primary\nmodel-policies:\n  - match: {alias: primary}\n  - match: {alias: backup}\n---\nReview changes.",
        &[],
        &config,
    );
    if let Some(local) = local {
        std::fs::write(root.join("mars.local.toml"), local).unwrap();
    }
    let mut command = mars_cmd(&root, temp.path(), &server.url(API_PATH));
    command
        .args(["build", "launch-bundle", "--agent", "reviewer"])
        .args(args)
        .env("PATH", replace_path_with(&bin_dir))
        .env("PROBE_LOG", &log);
    let output = command.output().unwrap();
    let calls = if log.exists() {
        std::fs::read_to_string(log).unwrap()
    } else {
        String::new()
    };
    (output, calls)
}

#[test]
fn explicit_empty_targets_do_not_autodiscover() {
    let (output, calls) = launch_with_scope("targets = []", None, &[]);
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("model fallback candidates exhausted")
            && error.contains("on linked harnesses"),
        "{error}"
    );
}

#[test]
fn generic_and_path_targets_enable_no_harnesses() {
    let (output, calls) =
        launch_with_scope("targets = [\".agents\", \"output/agents\"]", None, &[]);
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("model fallback candidates exhausted")
            && error.contains("on linked harnesses"),
        "{error}"
    );
}

#[test]
fn generic_managed_root_enables_no_harnesses() {
    let (output, calls) = launch_with_scope("managed_root = \".agents\"", None, &[]);
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("model fallback candidates exhausted")
            && error.contains("on linked harnesses"),
        "{error}"
    );
}

#[test]
fn local_empty_targets_override_project_targets() {
    let (output, calls) = launch_with_scope(
        "targets = [\".claude\"]",
        Some("[settings]\ntargets = []"),
        &[],
    );
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("model fallback candidates exhausted")
            && error.contains("on linked harnesses"),
        "{error}"
    );
}

#[test]
fn excluded_profile_harness_cannot_escape_target_scope() {
    let (output, calls) = launch_with_scope("targets = [\".codex\"]", None, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bundle["routing"]["harness"], "codex");
    assert_eq!(bundle["routing"]["model_token"], "backup");
    assert_eq!(calls.trim(), "codex login status");
}

#[test]
fn explicit_harness_cannot_expand_target_scope() {
    let (output, calls) =
        launch_with_scope("targets = [\".codex\"]", None, &["--harness", "claude"]);
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(calls.is_empty(), "{calls}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("explicit_harness_excluded"));
}

#[test]
fn unset_targets_preserve_autodiscovery() {
    let (output, calls) = launch_with_scope("", None, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bundle["routing"]["harness"], "claude");
    assert!(calls.contains("claude auth status"));
}

#[test]
fn caller_exclusions_skip_profile_preference_before_auth() {
    let (output, calls) = launch_with_scope(
        "targets = [\".claude\", \".codex\"]",
        None,
        &["--exclude-harness", "claude", "--exclude-harness", "claude"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bundle["routing"]["harness"], "codex");
    assert_eq!(bundle["routing"]["model_token"], "backup");
    assert_eq!(calls.trim(), "codex login status");
}

#[test]
fn caller_exclusions_cannot_be_overridden_by_an_explicit_pin() {
    let (output, calls) = launch_with_scope(
        "targets = [\".claude\", \".codex\"]",
        None,
        &["--exclude-harness", "claude", "--harness", "claude"],
    );
    assert!(!output.status.success());
    assert!(calls.is_empty(), "{calls}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("explicit_harness_excluded"));
}

#[test]
fn caller_exclusions_do_not_expand_target_scope() {
    let (output, calls) = launch_with_scope(
        "targets = [\".claude\"]",
        None,
        &["--exclude-harness", "claude"],
    );
    assert!(!output.status.success());
    assert!(calls.is_empty(), "{calls}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("model fallback candidates exhausted")
    );
}

#[test]
fn caller_exclusions_do_not_unpin_an_explicit_model() {
    let (output, calls) = launch_with_scope(
        "targets = [\".claude\", \".codex\"]",
        None,
        &["--exclude-harness", "claude", "--model", "primary"],
    );
    assert!(!output.status.success());
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("no linked harness available for model `primary`"),
        "{error}"
    );
}

#[test]
fn caller_exclusions_reject_unknown_harness_names() {
    let (output, calls) = launch_with_scope("", None, &["--exclude-harness", "unknown"]);
    assert!(!output.status.success());
    assert!(calls.is_empty(), "{calls}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("invalid value") && error.contains("unknown"),
        "{error}"
    );
}

#[test]
fn caller_exclusions_can_deny_all_without_configured_targets() {
    let (output, calls) = launch_with_scope(
        "",
        None,
        &[
            "--exclude-harness",
            "claude",
            "--exclude-harness",
            "codex",
            "--exclude-harness",
            "pi",
            "--exclude-harness",
            "opencode",
            "--exclude-harness",
            "cursor",
        ],
    );
    assert!(!output.status.success());
    assert!(calls.is_empty(), "{calls}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("model fallback candidates exhausted")
    );
}
