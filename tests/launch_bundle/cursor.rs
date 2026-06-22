use super::common::{
    assert_prompt_surface_excludes, install_fake_harnesses, replace_path_with, setup_bundle_project,
};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_accepts_cursor_harness_flag() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
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
        "cursor",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert!(
        bundle["provenance"].get("harness_stability").is_none(),
        "cursor should not be marked experimental"
    );
}

pub(crate) fn build_launch_bundle_accepts_profile_cursor_harness() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: cursor
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("profile")
    );
    assert!(
        bundle["provenance"].get("harness_stability").is_none(),
        "cursor should not be marked experimental"
    );
}

pub(crate) fn build_launch_bundle_cursor_alias_preserves_cursor_passthrough_only() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["cursor"]);
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [root_skill]
tools:
  Read: allow
  Edit: deny
mcp-tools: [plugin:root]
harness-overrides:
  opencode:
    skills: [opencode_skill]
    tools:
      Write: allow
    mcp-tools: [plugin:opencode]
    native-config:
      opencode.only: true
  cursor:
    skills: [cursor_skill]
    tools:
      Bash: allow
      Agent: deny
    disallowed-tools: [Edit]
    mcp-tools: [plugin:cursor]
    native-config:
      cursor.only: true
      cursor.array: [alpha, beta]
---
Review code changes."#;
    let root_skill = "---\nname: root_skill\ndescription: Root\n---\nRoot skill content.";
    let opencode_skill =
        "---\nname: opencode_skill\ndescription: OpenCode\n---\nOpenCode skill content.";
    let cursor_skill = "---\nname: cursor_skill\ndescription: Cursor\n---\nRoot skill content.";

    let extra_toml = r#"[models.cursoralias]
model = "claude-opus-4-6"
harness = "cursor""#;

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[
            ("root_skill", root_skill),
            ("opencode_skill", opencode_skill),
            ("cursor_skill", cursor_skill),
        ],
        extra_toml,
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "cursoralias",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("cursoralias")
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
    assert!(
        bundle["provenance"].get("harness_stability").is_none(),
        "cursor should not be marked experimental"
    );
    assert_eq!(
        bundle["skills"]["loaded"][0]["name"].as_str(),
        Some("root_skill")
    );
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(bundle["tools"]["allowed"], serde_json::json!(["Read"]));
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["StrReplace"])
    );
    assert_eq!(bundle["tools"]["mcp"], serde_json::json!(["plugin:root"]));
    assert_eq!(
        bundle["execution_policy"]["native_config"],
        serde_json::json!({
            "skills": ["cursor_skill"],
            "tools": {"Bash": "allow", "Agent": "deny"},
            "disallowed-tools": ["Edit"],
            "mcp-tools": ["plugin:cursor"],
            "native-config": {
                "cursor.only": true,
                "cursor.array": ["alpha", "beta"]
            }
        })
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile")
    );

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("root_skill"));
    assert!(
        docs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Root skill content.")
    );
    assert_prompt_surface_excludes(
        &bundle,
        &[
            "Cursor skill content.",
            "OpenCode skill content.",
            "opencode.only",
            "cursor.only",
            "cursor.array",
        ],
    );
}
