use super::common::{assert_prompt_surface_excludes, setup_bundle_project};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental() {
    let temp = TempDir::new().unwrap();
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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["provenance"]["harness_stability"].as_str(),
        Some("experimental")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning.as_str().unwrap_or_default()
            == "Cursor is an experimental launch-bundle target. The contract may change without notice."
    }));
}

pub(crate) fn build_launch_bundle_accepts_profile_cursor_harness() {
    let temp = TempDir::new().unwrap();
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

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("cursor"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["harness_stability"].as_str(),
        Some("experimental")
    );
}

pub(crate) fn build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: codex
skills: [root_skill]
tools:
  read: allow
  edit: deny
mcp-tools: [plugin:root]
harness-overrides:
  opencode:
    skills: [opencode_skill]
    tools:
      write: allow
    mcp-tools: [plugin:opencode]
    native-config:
      opencode.only: true
  cursor:
    skills: [cursor_skill]
    tools:
      bash: allow
      agent: deny
    disallowed-tools: [edit]
    mcp-tools: [plugin:cursor]
    native-config:
      cursor.only: true
      cursor.array: [alpha, beta]
---
Review code changes."#;
    let root_skill = "---\nname: root_skill\ndescription: Root\n---\nRoot skill content.";
    let opencode_skill =
        "---\nname: opencode_skill\ndescription: OpenCode\n---\nOpenCode skill content.";
    let cursor_skill = "---\nname: cursor_skill\ndescription: Cursor\n---\nCursor skill content.";

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
    assert_eq!(
        bundle["provenance"]["harness_stability"].as_str(),
        Some("experimental")
    );
    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["cursor_skill"])
    );
    assert_eq!(bundle["tools"]["allowed"], serde_json::json!(["Bash"]));
    assert_eq!(
        bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit"])
    );
    assert_eq!(bundle["tools"]["mcp"], serde_json::json!(["plugin:cursor"]));
    assert_eq!(
        bundle["execution_policy"]["native_config"],
        serde_json::json!({
            "cursor.only": true,
            "cursor.array": ["alpha", "beta"]
        })
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile-harness-override")
    );

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("cursor_skill"));
    assert!(
        docs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Cursor skill content.")
    );
    assert_prompt_surface_excludes(
        &bundle,
        &[
            "Root skill content.",
            "OpenCode skill content.",
            "opencode.only",
            "cursor.only",
            "cursor.array",
        ],
    );
}
