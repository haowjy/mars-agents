//! Native auth uncertainty and harness-default launches use the routing assessor.
#[path = "common/mod.rs"]
mod test_common;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn no_model_launch_rejects_logged_out_native_harness() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    #[cfg(windows)]
    let claude = bin.join("claude.bat");
    #[cfg(not(windows))]
    let claude = bin.join("claude");
    let script = std::fs::read_to_string(&claude).unwrap();
    std::fs::write(
        &claude,
        script
            .replace("exit 0", "exit 1")
            .replace("exit /b 0", "exit /b 1"),
    )
    .unwrap();
    std::fs::write(
        root.join("mars.toml"),
        "[settings]\ntargets=[\".claude\",\".codex\"]\nharness_order=[\"claude\",\"codex\"]\n",
    )
    .unwrap();
    let log = root.join("commands.log");
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(["build", "launch-bundle", "--no-refresh-models", "--json"])
        .env("PATH", &bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["claude auth status", "codex login status"]
    );
}

#[test]
fn unknown_native_auth_preserves_an_unverified_model_route() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    #[cfg(windows)]
    std::fs::write(
        bin.join("claude.bat"),
        "@echo off\r\n:waiting\r\ngoto waiting\r\n",
    )
    .unwrap();
    #[cfg(not(windows))]
    std::fs::write(bin.join("claude"), "#!/bin/sh\nexec /bin/sleep 5\n").unwrap();
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"}]}"#,
    )
    .unwrap();
    std::fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".claude\"]\n[models.opus]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n").unwrap();
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args(["models", "resolve", "opus", "--no-refresh-models", "--json"])
        .env("PATH", &bin)
        .env("PROBE_LOG", root.join("commands.log"))
        .env("MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS", "1")
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{value}");
    assert_eq!(value["harness"], "claude", "{value}");
    assert_eq!(value["availability"], "unknown", "{value}");
    let assessment = &value["route_trace"]["assessments"][0];
    assert_eq!(assessment["verdict"], "unverified", "{value}");
    assert_eq!(assessment["reason"], "auth_unknown", "{value}");
    assert!(assessment["skip_reason"].is_null(), "{value}");
}

#[test]
fn live_aliases_share_native_auth_evidence_for_the_invocation() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    std::fs::create_dir(root.join(".mars")).unwrap();
    std::fs::write(
        root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"}]}"#,
    )
    .unwrap();
    std::fs::write(root.join("mars.toml"), "[settings]\ntargets=[\".claude\"]\n[models.first]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n[models.second]\nmodel=\"claude-opus-4-6\"\nprovider=\"anthropic\"\n").unwrap();
    let log = root.join("commands.log");
    let output = test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args([
            "models",
            "list",
            "--live",
            "--unavailable",
            "--no-refresh-models",
            "--json",
        ])
        .env("PATH", &bin)
        .env("PROBE_LOG", &log)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["aliases"].as_array().unwrap().len(), 2);
    assert_eq!(
        std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["claude auth status"]
    );
}

fn profile_model_selection(primary: &str, targets: &[&str], args: &[&str]) -> Value {
    profile_model_selection_with(primary, targets, args, |_, _| {})
}

fn profile_model_selection_with(
    primary: &str,
    targets: &[&str],
    args: &[&str],
    customize: impl FnOnce(&std::path::Path, &std::path::Path),
) -> Value {
    let output = profile_model_output(primary, targets, args, customize);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn profile_model_output(
    primary: &str,
    targets: &[&str],
    args: &[&str],
    customize: impl FnOnce(&std::path::Path, &std::path::Path),
) -> std::process::Output {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let bin = test_common::install_logging_harnesses(root);
    std::fs::create_dir_all(root.join(".mars/agents")).unwrap();
    std::fs::write(
        root.join(".mars/agents/fixture.md"),
        format!(
            "---\nname: fixture\nmodel: {primary}\nmodel-policies:\n  - match: {{alias: primary}}\n    override: {{effort: low}}\n  - match: {{alias: second}}\n    override: {{effort: medium}}\n  - match: {{alias: backup}}\n    override: {{effort: high}}\n---\nWork.\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(".mars/models-cache.json"),
        r#"{"fetched_at":null,"models":[{"id":"claude-opus-4-6","provider":"anthropic"},{"id":"claude-sonnet-4-6","provider":"anthropic"},{"id":"gpt-5","provider":"openai"}]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mars.toml"),
        format!(
            r#"[settings]
targets={}
harness_order=["opencode","codex"]
[models.primary]
model="claude-opus-4-6"
provider="anthropic"
[models.second]
model="claude-sonnet-4-6"
provider="anthropic"
[models.backup]
model="gpt-5"
provider="openai"
"#,
            serde_json::to_string(targets).unwrap()
        ),
    )
    .unwrap();
    customize(root, &bin);
    test_common::mars_cmd(root, root, "http://127.0.0.1:1")
        .args([
            "build",
            "launch-bundle",
            "--agent",
            "fixture",
            "--no-refresh-models",
            "--json",
        ])
        .args(args)
        .env("PATH", &bin)
        .env("PROBE_LOG", root.join("commands.log"))
        .output()
        .unwrap()
}

#[test]
fn eligible_backup_beats_unverified_primary_and_earlier_backup() {
    let value = profile_model_selection("primary", &[".codex", ".opencode"], &[]);
    assert_eq!(value["routing"]["model_token"], "backup", "{value}");
    assert_eq!(value["routing"]["model"], "gpt-5", "{value}");
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(value["routing"]["harness_model"], "gpt-5", "{value}");
    assert_eq!(value["execution_policy"]["effort"], "high", "{value}");
    assert_eq!(
        value["provenance"]["model_source"], "profile-model-policy",
        "{value}"
    );
    assert_eq!(
        value["provenance"]["model_fallback_from"], "primary",
        "{value}"
    );
    assert_eq!(
        value["provenance"]["model_fallback_to"], "backup",
        "{value}"
    );
}

#[test]
fn first_unverified_model_is_retained_when_no_model_has_an_eligible_route() {
    let value = profile_model_selection("primary", &[".opencode"], &[]);
    assert_eq!(value["routing"]["model_token"], "primary", "{value}");
    assert_eq!(value["routing"]["model"], "claude-opus-4-6", "{value}");
    assert_eq!(value["execution_policy"]["effort"], "low", "{value}");
    assert_eq!(
        value["provenance"]["model_fallback_applied"], "false",
        "{value}"
    );
}

#[test]
fn explicit_model_pin_does_not_search_eligible_backup_models() {
    let value =
        profile_model_selection("primary", &[".codex", ".opencode"], &["--model", "primary"]);
    assert_eq!(value["routing"]["model_token"], "primary", "{value}");
    assert_eq!(value["routing"]["harness"], "opencode", "{value}");
    assert_eq!(value["provenance"]["model_source"], "cli", "{value}");
    assert_eq!(
        value["provenance"]["model_fallback_applied"], "false",
        "{value}"
    );
}

#[test]
fn eligible_primary_is_not_replaced_by_an_earlier_profile_entry() {
    let value = profile_model_selection("backup", &[".codex", ".opencode"], &[]);
    assert_eq!(value["routing"]["model_token"], "backup", "{value}");
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(value["provenance"]["model_source"], "profile", "{value}");
    assert_eq!(
        value["provenance"]["model_fallback_applied"], "false",
        "{value}"
    );
}

#[test]
fn later_model_mismatch_never_promotes_a_cleared_harness_default() {
    for with_eligible_backup in [false, true] {
        let value = profile_model_selection_with(
            "primary",
            &[".codex", ".opencode"],
            &[],
            |root, _| {
                let profile_path = root.join(".mars/agents/fixture.md");
                let mut profile = std::fs::read_to_string(&profile_path)
                    .unwrap()
                    .replace("match: {alias: second}", "match: {model: unknown-model}");
                if !with_eligible_backup {
                    profile = profile.replace(
                        "  - match: {alias: backup}\n    override: {effort: high}\n",
                        "",
                    );
                }
                std::fs::write(profile_path, profile).unwrap();
                let config_path = root.join("mars.toml");
                let mut config = std::fs::read_to_string(&config_path).unwrap();
                config.push_str("\n[[agents.fixture.model-policies]]\nmatch={model=\"unknown-model\"}\noverride={harness=\"codex\"}\n");
                std::fs::write(config_path, config).unwrap();
            },
        );
        let (token, model, effort) = if with_eligible_backup {
            ("backup", "gpt-5", "high")
        } else {
            ("primary", "claude-opus-4-6", "low")
        };
        assert_eq!(value["routing"]["model_token"], token, "{value}");
        assert_eq!(value["routing"]["model"], model, "{value}");
        assert_eq!(value["execution_policy"]["effort"], effort, "{value}");
        assert!(
            !value["warnings"].to_string().contains("clearing model"),
            "{value}"
        );
    }
}

#[test]
fn later_implicit_auth_rejection_preserves_the_first_unverified_attempt() {
    let value =
        profile_model_selection_with("primary", &[".codex", ".opencode"], &[], |root, bin| {
            let config_path = root.join("mars.toml");
            let config = std::fs::read_to_string(&config_path)
                .unwrap()
                .replace("[models.backup]", "[models.backup]\nharness=\"codex\"");
            std::fs::write(config_path, config).unwrap();
            #[cfg(windows)]
            let codex = bin.join("codex.bat");
            #[cfg(not(windows))]
            let codex = bin.join("codex");
            let script = std::fs::read_to_string(&codex).unwrap();
            std::fs::write(
                codex,
                script
                    .replace("exit 0", "exit 1")
                    .replace("exit /b 0", "exit /b 1"),
            )
            .unwrap();
        });
    assert_eq!(value["routing"]["model_token"], "primary", "{value}");
    assert_eq!(value["execution_policy"]["effort"], "low", "{value}");
    assert_eq!(value["provenance"]["model_source"], "profile", "{value}");
    assert_eq!(
        value["provenance"]["model_fallback_applied"], "false",
        "{value}"
    );
}

#[test]
fn explicit_harness_pin_allows_only_backup_models_on_that_harness() {
    let value =
        profile_model_selection("primary", &[".codex", ".opencode"], &["--harness", "codex"]);
    assert_eq!(value["routing"]["model_token"], "backup", "{value}");
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(value["provenance"]["harness_source"], "cli", "{value}");
    assert_eq!(
        value["routing"]["route_trace"]["candidates_tried"],
        serde_json::json!(["codex"]),
        "{value}"
    );

    let value = profile_model_selection(
        "primary",
        &[".codex", ".opencode"],
        &["--harness", "opencode"],
    );
    assert_eq!(value["routing"]["model_token"], "primary", "{value}");
    assert_eq!(value["routing"]["harness"], "opencode", "{value}");
    assert_eq!(
        value["routing"]["route_trace"]["candidates_tried"],
        serde_json::json!(["opencode"]),
        "{value}"
    );
}

#[test]
fn explicit_model_pin_retains_independent_profile_harness_preference() {
    let value = profile_model_selection_with(
        "primary",
        &[".codex", ".opencode"],
        &["--model", "backup"],
        |root, _| {
            let profile_path = root.join(".mars/agents/fixture.md");
            let profile = std::fs::read_to_string(&profile_path)
                .unwrap()
                .replace("name: fixture", "name: fixture\nharness: codex");
            std::fs::write(profile_path, profile).unwrap();
            let config_path = root.join("mars.toml");
            let config = std::fs::read_to_string(&config_path)
                .unwrap()
                .replace("[models.backup]", "[models.backup]\nharness=\"opencode\"");
            std::fs::write(config_path, config).unwrap();
        },
    );
    assert_eq!(value["routing"]["model_token"], "backup", "{value}");
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(value["provenance"]["harness_source"], "profile", "{value}");
    assert_eq!(value["provenance"]["model_source"], "cli", "{value}");
    assert_eq!(value["routing"]["selection_kind"], "auto", "{value}");
}

#[test]
fn invalid_authored_harness_preferences_are_fatal() {
    let mut failures = Vec::new();
    for (file, layer) in [
        ("mars.toml", "overlay"),
        ("mars.local.toml", "overlay"),
        ("mars.toml", "overlay-policy"),
        ("mars.local.toml", "overlay-policy"),
        ("mars.toml", "settings-policy"),
        ("mars.local.toml", "settings-policy"),
        (".mars/agents/fixture.md", "profile-policy"),
    ] {
        let output = profile_model_output("primary", &[".codex", ".opencode"], &[], |root, _| {
            let profile_path = root.join(".mars/agents/fixture.md");
            let mut profile = std::fs::read_to_string(&profile_path).unwrap();
            if layer == "profile-policy" {
                profile = profile.replace(
                    "override: {effort: low}",
                    "override: {effort: low, harness: typo}",
                );
                std::fs::write(profile_path, profile).unwrap();
                return;
            }
            if layer == "settings-policy" {
                std::fs::write(
                    profile_path,
                    "---\nname: fixture\nmodel: primary\n---\nWork.\n",
                )
                .unwrap();
            }
            let declaration = match layer {
                "overlay" => "\n[agents.fixture]\nharness=\"typo\"\n",
                "overlay-policy" => {
                    "\n[[agents.fixture.model-policies]]\nmatch={alias=\"primary\"}\noverride={harness=\"typo\"}\n"
                }
                _ => {
                    "\n[[settings.model-policies]]\nmatch={alias=\"primary\"}\noverride={harness=\"typo\"}\n"
                }
            };
            let path = root.join(file);
            let mut config = std::fs::read_to_string(&path).unwrap_or_default();
            config.push_str(declaration);
            std::fs::write(path, config).unwrap();
        });
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() || !output.stdout.is_empty() || !stderr.contains("typo") {
            failures.push(format!(
                "{file}/{layer}: status={:?}, stderr={stderr}, stdout={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn authored_harness_preference_is_normalized_before_assessment() {
    let value = profile_model_selection_with("backup", &[".codex", ".opencode"], &[], |root, _| {
        let path = root.join("mars.local.toml");
        std::fs::write(path, "[agents.fixture]\nharness=\" CoDeX \"\n").unwrap();
    });
    assert_eq!(value["routing"]["harness"], "codex", "{value}");
    assert_eq!(value["provenance"]["harness_source"], "overlay", "{value}");
    assert_eq!(
        value["routing"]["route_trace"]["source"], "overlay",
        "{value}"
    );
}
