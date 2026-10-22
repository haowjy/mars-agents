// qa-validated: launch-bundle-test-cleanup

use super::common::{install_fake_harnesses, replace_path_with, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

#[test]
fn build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_passthrough() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  Bash: allow
  Agent: deny
  Edit: deny
  mcp(plugin:root): allow
disallowed-tools: [Write]
harness-overrides:
  codex:
    tools:
      "Bash(meridian spawn *)": allow
      Agent: deny
    disallowed-tools: [Edit]
    mcp-tools: [plugin:override]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut root_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    root_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "claude",
    ]);
    root_cmd.env("PATH", replace_path_with(&bin_dir));
    let root_output = root_cmd.assert().success().get_output().clone();
    let root_bundle: Value = serde_json::from_slice(&root_output.stdout).unwrap();
    assert_eq!(root_bundle["tools"]["allowed"], serde_json::json!(["Bash"]));
    assert_eq!(
        root_bundle["tools"]["disallowed"],
        serde_json::json!([
            "Agent",
            "SendMessage",
            "ListAgents",
            "Edit",
            "Write",
            "NotebookEdit"
        ])
    );
    assert_eq!(
        root_bundle["tools"]["mcp"],
        serde_json::json!(["mcp__plugin:root__*"])
    );

    let mut override_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    override_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
        "--model",
        "gpt-5",
    ]);
    override_cmd.env("PATH", replace_path_with(&bin_dir));
    let override_output = override_cmd.assert().success().get_output().clone();
    let override_bundle: Value = serde_json::from_slice(&override_output.stdout).unwrap();
    assert_eq!(
        override_bundle["tools"]["allowed"],
        serde_json::json!(["exec_command"])
    );
    assert_eq!(
        override_bundle["tools"]["disallowed"],
        serde_json::json!([
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
            "send_message",
            "followup_task",
            "interrupt_agent",
            "list_agents",
            "apply_patch"
        ])
    );
    assert_eq!(override_bundle["tools"]["mcp"], serde_json::json!([]));
    // An MCP ref Codex cannot represent is a gap in the rule, never a launch warning.
    let mcp_rule = rule(&override_bundle, "mcp", Some("plugin:root/*"));
    assert_eq!(mcp_rule["native_status"], "unknown");
    assert!(mcp_rule["note"].is_string(), "{mcp_rule}");
    assert_eq!(override_bundle["warnings"], serde_json::json!([]));
    assert_eq!(
        override_bundle["execution_policy"]["native_config"]["mcp-tools"],
        serde_json::json!(["plugin:override"])
    );
}

#[test]
fn build_launch_bundle_normalizes_tool_head_and_preserves_scoped_payload() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  "Bash(git status *)": allow
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
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash(git status *)"])
    );
}

#[test]
fn build_launch_bundle_warns_for_unknown_first_class_tool_and_preserves_mcp() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  PlanMode: deny
  CustomDeny: deny
  CustomTool: allow
  Notebook: allow
  mcp(plugin:context7:context7): allow
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
        "claude",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    // `CustomTool` / `CustomDeny` are not capabilities: Mars leaves them out and says so
    // instead of guessing a Claude spelling.
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["NotebookEdit"])
    );
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["EnterPlanMode", "ExitPlanMode"])
    );
    assert_eq!(
        bundle["tools"]["mcp"],
        serde_json::json!(["mcp__plugin:context7:context7__*"])
    );
    for (name, action) in [("custom_tool", "allow"), ("custom_deny", "deny")] {
        let unknown = rule(&bundle, name, None);
        assert_eq!(unknown["action"], action);
        assert_eq!(unknown["native_status"], "unknown");
        assert_eq!(unknown["native"], serde_json::json!([]));
        assert!(unknown["note"].is_string(), "{unknown}");
    }
    assert_eq!(bundle["warnings"], serde_json::json!([]));
}

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
fn build_launch_bundle_cursor_and_pi_unknown_tools_are_left_out_with_a_note() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor", "pi"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  CustomTool: allow
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cursor_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cursor_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "cursor",
    ]);
    cursor_cmd.env("PATH", replace_path_with(&bin_dir));
    let cursor_output = cursor_cmd.assert().success().get_output().clone();
    let cursor_bundle: Value = serde_json::from_slice(&cursor_output.stdout).unwrap();
    assert_eq!(cursor_bundle["tools"]["allowed"], serde_json::json!([]));
    let cursor_rule = rule(&cursor_bundle, "custom_tool", None);
    assert_eq!(cursor_rule["native_status"], "unknown");
    assert!(cursor_rule["note"].is_string(), "{cursor_rule}");
    assert_eq!(cursor_bundle["warnings"], serde_json::json!([]));

    let mut pi_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    pi_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "pi",
    ]);
    pi_cmd.env("PATH", replace_path_with(&bin_dir));
    let pi_output = pi_cmd.assert().success().get_output().clone();
    let pi_bundle: Value = serde_json::from_slice(&pi_output.stdout).unwrap();
    assert_eq!(pi_bundle["tools"]["allowed"], serde_json::json!([]));
    let pi_rule = rule(&pi_bundle, "custom_tool", None);
    assert_eq!(pi_rule["native_status"], "unknown");
    assert!(pi_rule["note"].is_string(), "{pi_rule}");
    assert_eq!(pi_bundle["warnings"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_projects_mcp_refs_per_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools: [mcp(github/create_issue)]
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

#[test]
fn build_launch_bundle_projects_disallowed_mcp_refs_per_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor", "codex"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
disallowed-tools: [mcp(github/delete_repo)]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cursor_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cursor_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "cursor",
    ]);
    cursor_cmd.env("PATH", replace_path_with(&bin_dir));
    let cursor_output = cursor_cmd.assert().success().get_output().clone();
    let cursor_bundle: Value = serde_json::from_slice(&cursor_output.stdout).unwrap();
    assert_eq!(cursor_bundle["tools"]["mcp"], serde_json::json!([]));
    assert_eq!(
        cursor_bundle["tools"]["disallowed"],
        serde_json::json!(["Mcp(github:delete_repo)"])
    );
    assert_eq!(cursor_bundle["warnings"], serde_json::json!([]));

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
    assert_eq!(codex_bundle["tools"]["disallowed"], serde_json::json!([]));
    let codex_rule = rule(&codex_bundle, "mcp", Some("github/delete_repo"));
    assert_eq!(codex_rule["action"], "deny");
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
fn bundle_scopes_claude_write_with_edit_and_explains_the_effect() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  'write(src/**)': allow
---
Review code changes."#;
    let bundle = bundle_for(agent, "claude");
    let scoped_write = rule(&bundle, "write", Some("src/**"));
    assert_eq!(scoped_write["native"], serde_json::json!(["Edit(src/**)"]));
    assert_eq!(scoped_write["native_tools"], serde_json::json!(["Write"]));
    assert_eq!(scoped_write["native_status"], "known");
    let note = scoped_write["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("allow adds no practical write power"),
        "{note}"
    );
    assert!(
        note.contains("deny also refuses other built-in file edits"),
        "{note}"
    );
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Edit(src/**)"])
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
fn bundle_marks_tools_a_harness_lacks_as_absent() {
    let codex = bundle_for(TYPED_RULES_AGENT, "codex");
    // Codex has no notebook or lsp tool, and the model reads files through the shell.
    for capability in ["notebook", "lsp"] {
        let absent = rule(&codex, capability, None);
        assert_eq!(absent["native_status"], "absent", "{capability}");
        assert_eq!(absent["native"], serde_json::json!([]));
        assert_eq!(absent["native_tools"], serde_json::json!([]));
    }
    assert_eq!(
        rule(&codex, "edit", None)["native"],
        serde_json::json!(["apply_patch"])
    );
    assert_eq!(
        rule(&codex, "edit", None)["native_tools"],
        serde_json::json!(["apply_patch"])
    );
    assert_eq!(
        rule(&codex, "web", None)["native"],
        serde_json::json!(["web_search"])
    );
    assert_eq!(
        rule(&codex, "mcp", Some("github/delete_repo"))["native_status"],
        "unknown"
    );

    let opencode = bundle_for(TYPED_RULES_AGENT, "opencode");
    assert_eq!(
        rule(&opencode, "ask_user", None)["native"],
        serde_json::json!(["question"])
    );
    assert_eq!(
        rule(&opencode, "ask_user", None)["native_tools"],
        serde_json::json!(["question"])
    );
    assert_eq!(
        rule(&opencode, "agent", None)["native"],
        serde_json::json!(["task"])
    );
    assert_eq!(
        rule(&opencode, "task", None)["native"],
        serde_json::json!(["todowrite"])
    );
    assert_eq!(
        rule(&opencode, "web", None)["native"],
        serde_json::json!(["websearch", "webfetch"])
    );
}

#[test]
fn bundle_list_form_tools_is_an_allowlist_with_an_explicit_wildcard_deny() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools: [read, grep]
---
Review code changes."#;
    let bundle = bundle_for(agent, "claude");
    assert_eq!(
        bundle["tools"]["rules"],
        serde_json::json!([
            {"capability": "*", "pattern": null, "action": "deny",
             "native": [], "native_tools": [], "origin": "list-form"},
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

/// The shape of the shipped `prompt-tester`: no editing, but writing files is fine.
const NO_EDIT_BUT_WRITE_AGENT: &str = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  bash: allow
  read: allow
  write: allow
  edit: deny
  notebook: deny
---
Test prompts."#;

#[test]
fn bundle_resolves_a_narrow_rule_over_a_broad_one_and_notes_where_it_cannot() {
    // Claude and Pi tell `write` from `edit`: the narrow allow wins the shared tool.
    for (harness, allowed, denied) in [("claude", "Write", "Edit"), ("pi", "write", "edit")] {
        let bundle = bundle_for(NO_EDIT_BUT_WRITE_AGENT, harness);
        let allowed_list = bundle["tools"]["allowed"].as_array().unwrap();
        let disallowed_list = bundle["tools"]["disallowed"].as_array().unwrap();
        assert!(
            allowed_list.contains(&serde_json::json!(allowed)),
            "{harness}"
        );
        assert!(
            !disallowed_list.contains(&serde_json::json!(allowed)),
            "{harness}"
        );
        assert!(
            disallowed_list.contains(&serde_json::json!(denied)),
            "{harness}"
        );
        assert!(rule(&bundle, "write", None)["note"].is_null(), "{harness}");
        assert!(rule(&bundle, "edit", None)["note"].is_null(), "{harness}");
        assert_eq!(
            rule(&bundle, "write", None)["native_tools"],
            serde_json::json!([allowed]),
            "{harness}"
        );
    }
    let claude = bundle_for(NO_EDIT_BUT_WRITE_AGENT, "claude");
    assert_eq!(
        rule(&claude, "edit", None)["native"],
        serde_json::json!(["Edit", "NotebookEdit"])
    );
    assert_eq!(
        rule(&claude, "edit", None)["native_tools"],
        serde_json::json!(["Edit", "NotebookEdit"])
    );

    // Codex edits and writes through `apply_patch` alone, so Mars cannot pick: both rules say the
    // harness decides, and the launch gets no warning.
    let codex = bundle_for(NO_EDIT_BUT_WRITE_AGENT, "codex");
    for capability in ["write", "edit"] {
        let overlapping = rule(&codex, capability, None);
        assert_eq!(overlapping["native_status"], "known");
        assert_eq!(
            overlapping["native_tools"],
            serde_json::json!(["apply_patch"])
        );
        assert!(
            overlapping["note"]
                .as_str()
                .is_some_and(|note| note.contains("the harness decides")),
            "{overlapping}"
        );
    }
    assert_eq!(codex["warnings"], serde_json::json!([]));
}

#[test]
fn bundle_applies_the_tools_overlay_and_keeps_the_profiles_denies() {
    let agent = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  read: allow
  'bash(git push *)': deny
disallowed-tools: [agent]
---
Review code changes."#;
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent,
        &[],
        "[agents.reviewer.tools]\nallowed = [\"bash\", \"write\"]",
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
        serde_json::json!(["Bash", "Write"])
    );
    let disallowed = bundle["tools"]["disallowed"].to_string();
    assert!(
        disallowed.contains("Bash(git push *)") && disallowed.contains("Agent"),
        "{disallowed}"
    );
    let wildcard = rule(&bundle, "*", None);
    assert_eq!(
        wildcard["origin"], "list-form",
        "an overlay allowlist is list-form"
    );
}
