// qa-validated: launch-bundle-test-cleanup

use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

#[test]
fn build_launch_bundle_opencode_tool_normalization_maps_to_opencode_permission_names() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["opencode"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  Read: allow
  Write: allow
  CustomTool: allow
  Edit: deny
  Agent: deny
  WebSearch: allow
  WebFetch: allow
  PlanMode: deny
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "opencode",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["bash", "read", "edit", "websearch", "webfetch"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["edit", "task", "plan_enter", "plan_exit"])
    );

    assert_eq!(
        rule(&bundle, "custom_tool", None)["native_status"],
        "unknown"
    );
    assert_eq!(bundle["warnings"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_skill_deny_is_a_gap_on_harnesses_that_load_skills_by_file() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  'skill(init)': deny
  workflow: deny
  web: allow
  definitely-not-a-tool: deny
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut claude_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    claude_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    claude_cmd.env("PATH", replace_path_with(&bin_dir));
    let claude_output = claude_cmd.assert().success().get_output().clone();
    let claude_bundle: Value = serde_json::from_slice(&claude_output.stdout).unwrap();
    assert_eq!(
        claude_bundle["tools"]["allowed"],
        serde_json::json!(["Bash", "WebSearch", "WebFetch"])
    );
    assert_eq!(
        claude_bundle["tools"]["disallowed"],
        serde_json::json!(["Skill(init)", "Workflow"])
    );
    let unknown = rule(&claude_bundle, "definitely-not-a-tool", None);
    assert_eq!(unknown["native_status"], "unknown");
    assert!(unknown["note"].is_string(), "{unknown}");
    assert_eq!(claude_bundle["warnings"], serde_json::json!([]));

    let mut codex_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    codex_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    codex_cmd.env("PATH", replace_path_with(&bin_dir));
    let codex_output = codex_cmd.assert().success().get_output().clone();
    let codex_bundle: Value = serde_json::from_slice(&codex_output.stdout).unwrap();
    assert_eq!(
        codex_bundle["tools"]["allowed"],
        serde_json::json!(["exec_command", "web_search"])
    );
    // Codex has no workflow tool, so that deny is satisfied. It loads skills by reading files, so
    // `skill(init)` has no tool to switch: a reported gap with its reason in `note`, not a
    // satisfied rule and not a launch warning.
    assert_eq!(codex_bundle["tools"]["disallowed"], serde_json::json!([]));
    let skill_rule = codex_bundle["tools"]["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|rule| rule["capability"] == "skill")
        .expect("skill rule");
    assert_eq!(skill_rule["native_status"], "unknown");
    assert_eq!(
        skill_rule["note"],
        "codex loads skills by reading files; no switch"
    );
    assert_eq!(
        rule(&codex_bundle, "definitely-not-a-tool", None)["native_status"],
        "unknown"
    );
    assert_eq!(codex_bundle["warnings"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_projects_mcp_refs_per_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  mcp(github/create_issue): allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut claude_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    claude_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    claude_cmd.env("PATH", replace_path_with(&bin_dir));
    let claude_output = claude_cmd.assert().success().get_output().clone();
    let claude_bundle: Value = serde_json::from_slice(&claude_output.stdout).unwrap();
    assert_eq!(
        claude_bundle["tools"]["mcp"],
        serde_json::json!(["mcp__github__create_issue"])
    );
    assert_eq!(
        rule(&claude_bundle, "mcp", Some("github/create_issue"))["native_status"],
        "known"
    );
    assert_eq!(claude_bundle["warnings"], serde_json::json!([]));

    let mut codex_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    codex_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    codex_cmd.env("PATH", replace_path_with(&bin_dir));
    let codex_output = codex_cmd.assert().success().get_output().clone();
    let codex_bundle: Value = serde_json::from_slice(&codex_output.stdout).unwrap();
    assert_eq!(codex_bundle["tools"]["mcp"], serde_json::json!([]));
    // A ref Codex cannot represent is a gap in the rule, not a launch warning.
    let codex_rule = rule(&codex_bundle, "mcp", Some("github/create_issue"));
    assert_eq!(codex_rule["native_status"], "unknown");
    assert!(codex_rule["note"].is_string(), "{codex_rule}");
    assert_eq!(codex_bundle["warnings"], serde_json::json!([]));
}

/// Launch bundle for `agent_content` on `harness`.
fn bundle_for(agent_content: &str, harness: &str) -> Value {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &[harness]);
    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        harness,
    ]);
    if harness == "codex" {
        cmd.args(["--model", "gpt-5"]);
    }
    cmd.env("PATH", replace_path_with(&bin_dir));
    let output = cmd.assert().success().get_output().clone();
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rule<'a>(bundle: &'a Value, capability: &str, pattern: Option<&str>) -> &'a Value {
    bundle["tools"]["rules"]
        .as_array()
        .expect("tools.rules")
        .iter()
        .find(|rule| {
            rule["capability"] == capability && rule["pattern"] == serde_json::json!(pattern)
        })
        .unwrap_or_else(|| panic!("no rule {capability} {pattern:?} in {}", bundle["tools"]))
}

const TYPED_RULES_AGENT: &str = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  'bash(rg *)': allow
  'bash(git push:*)': ask
  edit: deny
  web: deny
  ask_user: deny
  task: deny
  agent: deny
  notebook: deny
  lsp: deny
  mcp(github/delete_repo): deny
  mystery_tool: deny
---
Review code changes."#;

#[test]
fn bundle_tools_rules_carry_capability_and_native_names_apart() {
    let bundle = bundle_for(TYPED_RULES_AGENT, "claude");
    assert_eq!(bundle["version"], 4, "rules are additive in schema 4");

    assert_eq!(
        *rule(&bundle, "edit", None),
        serde_json::json!({
            "capability": "edit", "pattern": null, "action": "deny",
            "native": ["Edit", "Write", "NotebookEdit"],
            "native_tools": ["Edit", "Write", "NotebookEdit"],
            "native_status": "known"
        })
    );
    assert_eq!(
        *rule(&bundle, "bash", Some("rg *")),
        serde_json::json!({
            "capability": "bash", "pattern": "rg *", "action": "allow",
            "native": ["Bash(rg *)"], "native_tools": ["Bash"],
            "native_status": "known"
        })
    );
    assert_eq!(
        rule(&bundle, "web", None)["native"],
        serde_json::json!(["WebSearch", "WebFetch"])
    );
    assert_eq!(
        rule(&bundle, "ask_user", None)["native"],
        serde_json::json!(["AskUserQuestion"])
    );
    // `task` (task list) and `agent` (sub-agents) stay two rules.
    assert_eq!(
        rule(&bundle, "agent", None)["native"],
        serde_json::json!(["Agent", "SendMessage", "ListAgents"])
    );
    assert_eq!(
        rule(&bundle, "task", None)["native"],
        serde_json::json!([
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskOutput",
            "TaskUpdate",
            "TaskStop",
            "TodoWrite"
        ])
    );
    assert_eq!(
        *rule(&bundle, "mcp", Some("github/delete_repo")),
        serde_json::json!({
            "capability": "mcp", "pattern": "github/delete_repo", "action": "deny",
            "native": ["mcp__github__delete_repo"],
            "native_tools": ["mcp__github__delete_repo"], "native_status": "known"
        })
    );
    // A name Mars cannot map is a reported gap, never a guess, and it still draws a warning.
    assert_eq!(
        *rule(&bundle, "mystery_tool", None),
        serde_json::json!({
            "capability": "mystery_tool", "pattern": null, "action": "deny",
            "native": [], "native_tools": [], "native_status": "unknown",
            "note": "no native name known for this tool on this harness; left out"
        })
    );
}

#[test]
fn bundle_accepts_ask_and_normalizes_colon_star() {
    let bundle = bundle_for(TYPED_RULES_AGENT, "claude");
    let ask = rule(&bundle, "bash", Some("git push *"));
    assert_eq!(ask["action"], "ask");
    assert_eq!(ask["native"], serde_json::json!(["Bash(git push *)"]));
    assert!(
        !bundle["tools"]["rules"].to_string().contains(":*"),
        "{}",
        bundle["tools"]["rules"]
    );
}

#[test]
fn bundle_flat_lists_derive_from_rules_and_cannot_hold_ask() {
    let bundle = bundle_for(TYPED_RULES_AGENT, "claude");
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash(rg *)"])
    );
    let disallowed = bundle["tools"]["disallowed"].as_array().unwrap();
    assert!(disallowed.contains(&serde_json::json!("AskUserQuestion")));
    assert!(disallowed.contains(&serde_json::json!("mcp__github__delete_repo")));
    assert!(
        !disallowed
            .iter()
            .any(|name| name.as_str().unwrap().contains("git push")),
        "an ask rule is neither allowed nor disallowed: {disallowed:?}"
    );
}

#[test]
fn bundle_explicit_allowlist_carries_the_wildcard_deny() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  '*': deny
  read: allow
  grep: allow
---
Review code changes."#;
    let bundle = bundle_for(agent, "claude");
    assert_eq!(
        bundle["tools"]["rules"],
        serde_json::json!([
            {"capability": "*", "pattern": null, "action": "deny",
             "native": [], "native_tools": []},
            {"capability": "read", "pattern": null, "action": "allow",
             "native": ["Read"], "native_tools": ["Read"], "native_status": "known"},
            {"capability": "grep", "pattern": null, "action": "allow",
             "native": ["Grep"], "native_tools": ["Grep"], "native_status": "known"},
        ])
    );
    // The flat lists never carried an allowlist, and still do not.
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Read", "Grep"])
    );
    assert_eq!(bundle["tools"]["disallowed"], serde_json::json!([]));
}

#[test]
fn bundle_warns_for_each_project_local_plain_tool_list() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  '*': deny
---
Review code changes."#;
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent,
        &[],
        "[agents.reviewer.tools]\nallowed = [\"Bash\"]",
    );
    std::fs::write(
        project_root.join("mars.local.toml"),
        "[agents.reviewer.tools]\nallowed = [\"Read\"]\n",
    )
    .unwrap();

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash", "Read"])
    );
    let warnings = bundle["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|message| {
                message.contains("mars.toml [agents.reviewer.tools].allowed")
                    && message.contains("mars.local.toml")
                    && message.contains("allowed = { replace = [...] }")
            })),
        "{}",
        bundle["warnings"]
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().is_some_and(|message| {
                message.contains("mars.local.toml [agents.reviewer.tools].allowed")
                    && message.contains("Mars 0.17")
            })),
        "{}",
        bundle["warnings"]
    );
}

#[test]
fn authored_empty_overlay_list_keeps_the_017_result_without_warning() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Read: allow
  Grep: allow
  Write: deny
---
Review code changes."#;
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent,
        &[],
        "[agents.reviewer.tools]\nallowed = []",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));
    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Read", "Grep"])
    );
    assert_eq!(bundle["tools"]["disallowed"], serde_json::json!(["Write"]));
    assert!(
        bundle["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|warning| !warning
                .as_str()
                .is_some_and(|message| message.contains("tools].allowed"))),
        "{}",
        bundle["warnings"]
    );
}
