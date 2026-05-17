// qa-validated: launch-bundle-blocker-audit
mod common;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use httpmock::prelude::*;
use serde_json::Value;

use common::{API_PATH, create_source, mars_cmd, sample_catalog_json};

fn setup_bundle_project(
    temp: &TempDir,
    source_name: &str,
    agent_content: &str,
    skills: &[(&str, &str)],
    extra_project_toml: &str,
) -> (MockServer, std::path::PathBuf) {
    setup_bundle_project_with_agents(
        temp,
        source_name,
        &[("reviewer", agent_content)],
        skills,
        extra_project_toml,
    )
}

fn setup_bundle_project_with_agents(
    temp: &TempDir,
    source_name: &str,
    agents: &[(&str, &str)],
    skills: &[(&str, &str)],
    extra_project_toml: &str,
) -> (MockServer, std::path::PathBuf) {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });

    let source = create_source(temp, source_name, agents, skills);
    let project = temp.child("project");
    project.create_dir_all().unwrap();

    let mut toml = format!(
        "[dependencies]\n{source_name} = {{ path = \"{}\" }}\n",
        source.display().to_string().replace('\\', "/")
    );
    if !extra_project_toml.trim().is_empty() {
        toml.push('\n');
        toml.push_str(extra_project_toml);
        toml.push('\n');
    }
    project.child("mars.toml").write_str(&toml).unwrap();

    let mut sync_cmd = mars_cmd(project.path(), temp.path(), &server.url(API_PATH));
    sync_cmd.arg("sync");
    sync_cmd.assert().success();

    (server, project.to_path_buf())
}

fn assert_prompt_surface_excludes(bundle: &Value, needles: &[&str]) {
    let prompt_surface = &bundle["prompt_surface"];
    let mut surfaces = vec![
        (
            "system_instruction",
            prompt_surface["system_instruction"]
                .as_str()
                .unwrap_or_default(),
        ),
        (
            "inventory_prompt",
            prompt_surface["inventory_prompt"]
                .as_str()
                .unwrap_or_default(),
        ),
    ];

    let empty_docs = Vec::new();
    let docs = prompt_surface["supplemental_documents"]
        .as_array()
        .unwrap_or(&empty_docs);
    for doc in docs {
        surfaces.push((
            "supplemental_documents.content",
            doc["content"].as_str().unwrap_or_default(),
        ));
    }

    for needle in needles {
        for (surface_name, content) in &surfaces {
            assert!(
                !content.contains(needle),
                "`{needle}` leaked into prompt surface `{surface_name}`: {content}"
            );
        }
    }
}

#[test]
fn build_launch_bundle_outputs_schema_and_slot_placeholders() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
tools: [Bash, Write]
disallowed-tools: [Agent]
mcp-tools: [plugin:context7:context7]
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to plan.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();

    let bundle: Value = serde_json::from_str(&stdout).expect("launch-bundle should emit JSON");

    assert_eq!(bundle["version"].as_u64(), Some(1));
    assert_eq!(bundle["agent"].as_str(), Some("reviewer"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["tools"]["allowed"],
        serde_json::json!(["Bash", "Write"])
    );
    assert_eq!(bundle["tools"]["disallowed"], serde_json::json!(["Agent"]));
    assert_eq!(
        bundle["tools"]["mcp"],
        serde_json::json!(["plugin:context7:context7"])
    );
    assert!(bundle["provenance"]["harness_stability"].is_null());

    for slot in [
        "completion_contract",
        "context_prompt",
        "user_prompt_file",
        "context_files",
        "prior_session_context",
        "spawn_metadata",
    ] {
        assert_eq!(bundle["scaffold_slots"][slot].as_str(), Some("###SLOT###"));
    }
}

#[test]
fn build_launch_bundle_includes_skill_documents_and_system_instruction() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\n---\nUse this skill to reason about steps.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("planning"));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: planning"));
    assert!(system_instruction.contains("Use this skill to reason about steps."));
    assert!(system_instruction.contains("# Report"));
    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["planning"])
    );
    assert_eq!(bundle["skills_metadata"]["missing"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_rejects_prompt_file_flag() {
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
        "--prompt-file",
        "task.md",
    ]);

    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("--prompt-file"));
}

#[test]
fn build_launch_bundle_resolves_model_alias_from_consumer_config() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: bundlealias
---
Review code changes."#;

    let extra_toml = r#"[models.bundlealias]
model = "openai/gpt-5"
harness = "codex"
default_effort = "high"
autocompact = 24000
autocompact_pct = 70"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("bundlealias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai/gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["autocompact"].as_u64(),
        Some(24000)
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact_pct"].as_u64(),
        Some(70)
    );
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
}

#[test]
fn build_launch_bundle_uses_harness_variant_skill_for_codex() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
---
Review code changes."#;
    let skill_content = "---\nname: planning\ndescription: Plan tasks\ntype: guardrail\n---\nBase planning content.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let codex_variant_path = project_root
        .join(".mars")
        .join("skills")
        .join("planning")
        .join("variants")
        .join("codex");
    std::fs::create_dir_all(&codex_variant_path).unwrap();
    std::fs::write(
        codex_variant_path.join("SKILL.md"),
        "---\nname: planning\ndescription: Plan tasks\n---\nCodex variant content.",
    )
    .unwrap();

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["skill_type"].as_str(), Some("guardrail"));
    assert!(
        docs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Codex variant content.")
    );
}

#[test]
fn build_launch_bundle_uses_harness_override_skills_for_prompt_surface() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
harness-overrides:
  codex:
    skills: [codex_skill]
---
Review code changes."#;
    let planning_skill =
        "---\nname: planning\ndescription: Plan tasks\n---\nPlanning base content.";
    let codex_skill =
        "---\nname: codex_skill\ndescription: Codex helper\n---\nCodex-specific content.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", planning_skill), ("codex_skill", codex_skill)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("codex_skill"));
    assert!(
        docs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Codex-specific content.")
    );

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: codex_skill"));
    assert!(!system_instruction.contains("# Skill: planning"));

    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["codex_skill"])
    );
    assert_eq!(bundle["skills_metadata"]["missing"], serde_json::json!([]));
}

#[test]
fn build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental() {
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

#[test]
fn build_launch_bundle_accepts_profile_cursor_harness() {
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

#[test]
fn build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy() {
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

#[test]
fn build_launch_bundle_skips_model_non_invocable_skills() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning, hidden]
---
Review code changes."#;
    let planning_skill = "---\nname: planning\ndescription: Plan tasks\ntype: reference\n---\nVisible skill content.";
    let hidden_skill = "---\nname: hidden\ndescription: Hidden skill\nmodel-invocable: false\ntype: principle\n---\nShould not be present.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", planning_skill), ("hidden", hidden_skill)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("planning"));

    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["planning"])
    );
    assert_eq!(bundle["skills_metadata"]["missing"], serde_json::json!([]));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(!system_instruction.contains("Should not be present."));
}

#[test]
fn build_launch_bundle_includes_inventory_prompt_before_report_block() {
    let temp = TempDir::new().unwrap();
    let reviewer_content = r#"---
name: reviewer
description: Review implementation
mode: subagent
model: claude-opus-4-6
---
Review code changes."#;
    let planner_content = r#"---
name: planner
description: Plan tasks
mode: primary
model: openai/gpt-5
---
Plan work."#;

    let (server, project_root) = setup_bundle_project_with_agents(
        &temp,
        "bundle-source",
        &[("reviewer", reviewer_content), ("planner", planner_content)],
        &[],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let inventory_prompt = bundle["prompt_surface"]["inventory_prompt"]
        .as_str()
        .expect("inventory_prompt should be string");
    assert!(inventory_prompt.contains("# Meridian Agents"));
    assert!(inventory_prompt.contains("## Primary"));
    assert!(inventory_prompt.contains("## Subagent"));
    assert!(inventory_prompt.contains("- planner: Plan tasks | Model: openai/gpt-5"));
    assert!(
        inventory_prompt.contains("- reviewer: Review implementation | Model: claude-opus-4-6")
    );

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    let inventory_index = system_instruction
        .find("# Meridian Agents")
        .expect("system instruction should include inventory");
    let report_index = system_instruction
        .find("# Report")
        .expect("system instruction should include report block");
    assert!(inventory_index < report_index);
}

#[test]
fn build_launch_bundle_orders_skills_by_type_and_bookends_principles() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [reference_a, principle_a, guardrail_a, reference_b]
---
Review code changes."#;

    let reference_a =
        "---\nname: reference_a\ndescription: Ref A\ntype: reference\n---\nReference A body.";
    let principle_a =
        "---\nname: principle_a\ndescription: Principle A\ntype: principle\n---\nPrinciple body.";
    let guardrail_a =
        "---\nname: guardrail_a\ndescription: Guardrail A\ntype: guardrail\n---\nGuardrail body.";
    let reference_b =
        "---\nname: reference_b\ndescription: Ref B\ntype: unknown-kind\n---\nReference B body.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[
            ("reference_a", reference_a),
            ("principle_a", principle_a),
            ("guardrail_a", guardrail_a),
            ("reference_b", reference_b),
        ],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    let ordered_names = docs
        .iter()
        .map(|doc| doc["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ordered_names,
        vec!["principle_a", "guardrail_a", "reference_a", "reference_b"]
    );

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    let first_principle_index = system_instruction
        .find("# Skill: principle_a")
        .expect("system instruction should include principle skill");
    let report_index = system_instruction
        .find("# Report")
        .expect("system instruction should include report block");
    assert!(first_principle_index < report_index);

    let second_principle_relative = system_instruction[(report_index + "# Report".len())..]
        .find("# Skill: principle_a")
        .expect("system instruction should include trailing principle bookend");
    let second_principle_index = report_index + "# Report".len() + second_principle_relative;
    assert!(second_principle_index > report_index);
}

#[test]
fn build_launch_bundle_cli_model_alias_harness_beats_profile_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[models.bundlealias]
model = "openai/gpt-5"
harness = "codex""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "bundlealias",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("bundlealias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai/gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("alias")
    );
}

#[test]
fn build_launch_bundle_cli_model_override_uses_provider_harness_before_profile_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
model = "gpt-5""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--model",
        "openai_alias",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model_token"].as_str(),
        Some("openai_alias")
    );
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
}

#[test]
fn build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: openai_alias
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
model = "gpt-5""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
}

#[test]
fn build_launch_bundle_uses_alias_provider_when_auto_resolve_misses_model_cache() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: openai_alias
---
Review code changes."#;

    let extra_toml = r#"[models.openai_alias]
provider = "openai"
match = ["definitely-not-a-cached-openai-model-*"]"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model"].as_str(), Some("openai_alias"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("did not resolve from cached catalog")
    }));
}

#[test]
fn build_launch_bundle_uses_settings_default_harness_before_hardcoded_fallback() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_harness = "pi""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["routing"]["model"].as_str(),
        Some("unknown-model-token")
    );
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("pi"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("config")
    );
}

#[test]
fn build_launch_bundle_inventory_hides_model_non_invocable_agents_and_shows_fanout() {
    let temp = TempDir::new().unwrap();
    let reviewer_content = r#"---
name: reviewer
description: Review implementation
mode: subagent
model: claude-opus-4-6
model-policies:
  - match:
      alias: gpt55
    override: {}
  - match:
      alias: gpt55
    override: {}
  - match:
      model: gpt-5
    override: {}
  - match:
      alias: hidden
    no-fallback: true
    override: {}
---
Review code changes."#;
    let hidden_content = r#"---
name: hidden-worker
description: internal helper
mode: subagent
model: claude-opus-4-6
model-invocable: false
---
Hidden work."#;

    let (server, project_root) = setup_bundle_project_with_agents(
        &temp,
        "bundle-source",
        &[
            ("reviewer", reviewer_content),
            ("hidden-worker", hidden_content),
        ],
        &[],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let inventory_prompt = bundle["prompt_surface"]["inventory_prompt"]
        .as_str()
        .expect("inventory_prompt should be string");
    assert!(inventory_prompt.contains("reviewer: Review implementation"));
    assert!(inventory_prompt.contains("Fan-out: gpt55, gpt-5"));
    assert!(!inventory_prompt.contains("hidden-worker"));
}

#[test]
fn build_launch_bundle_fails_on_unknown_agent_harness() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: not-a-harness
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("unknown harness"));
}

#[test]
fn build_launch_bundle_fails_on_invalid_top_level_agent_field_value() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
model-invocable: nope
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("model-invocable"));
}

#[test]
fn build_launch_bundle_fails_on_non_overridable_model_invocable_override() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  claude:
    model-invocable: false
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("not overridable"));
}

#[test]
fn build_launch_bundle_fails_when_inventory_agent_has_fatal_frontmatter_diagnostic() {
    let temp = TempDir::new().unwrap();
    let reviewer_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;
    let malformed_inventory_content = r#"---
name: malformed
model: claude-opus-4-6
model-invocable: nope
---
Broken inventory entry."#;

    let (server, project_root) = setup_bundle_project_with_agents(
        &temp,
        "bundle-source",
        &[
            ("reviewer", reviewer_content),
            ("malformed", malformed_inventory_content),
        ],
        &[],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("inventory file"));
}

#[test]
fn build_launch_bundle_merges_extra_skills_after_profile_dedupes_and_tracks_missing() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning]
---
Review code changes."#;
    let planning_skill =
        "---\nname: planning\ndescription: Plan tasks\ntype: reference\n---\nPlanning content.";
    let extra_skill =
        "---\nname: extra_skill\ndescription: Extra helper\ntype: reference\n---\nExtra content.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", planning_skill), ("extra_skill", extra_skill)],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--skill",
        "planning,missing_skill,extra_skill,extra_skill",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["skills_metadata"]["loaded"],
        serde_json::json!(["planning", "extra_skill"])
    );
    assert_eq!(
        bundle["skills_metadata"]["missing"],
        serde_json::json!(["missing_skill"])
    );

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: planning"));
    assert!(system_instruction.contains("# Skill: extra_skill"));
    assert!(!system_instruction.contains("# Skill: missing_skill"));
}

#[test]
fn build_launch_bundle_cli_overrides_profile_execution_policy_fields() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: low
approval: confirm
sandbox: read-only
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
        "--effort",
        "high",
        "--approval",
        "yolo",
        "--sandbox",
        "danger-full-access",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("yolo")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("danger-full-access")
    );
    assert_eq!(bundle["provenance"]["effort_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(bundle["provenance"]["sandbox_source"].as_str(), Some("cli"));
}

#[test]
fn build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  codex:
    native-config:
      sandbox_workspace_write.network_access: true
      approval: "still native"
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
        "codex",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["execution_policy"]["native_config"],
        serde_json::json!({
            "sandbox_workspace_write.network_access": true,
            "approval": "still native"
        })
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile-harness-override")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("collides with a portable field name")
    }));

    assert_prompt_surface_excludes(
        &bundle,
        &["sandbox_workspace_write.network_access", "still native"],
    );
}

#[test]
fn build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: modelalias
effort: low
approval: confirm
sandbox: read-only
autocompact: 1200
autocompact_pct: 40
harness-overrides:
  codex:
    effort: high
    approval: auto
    sandbox: workspace-write
    autocompact: 2400
    autocompact_pct: 70
    native-config:
      sandbox_workspace_write.network_access: true
---
Review code changes."#;

    let extra_toml = r#"[models.modelalias]
model = "openai/gpt-5"
harness = "codex"
default_effort = "medium"
autocompact = 9000
autocompact_pct = 55"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--approval",
        "yolo",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("high"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("yolo"),
        "CLI approval must beat harness override",
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact"].as_u64(),
        Some(2400)
    );
    assert_eq!(
        bundle["execution_policy"]["autocompact_pct"].as_u64(),
        Some(70)
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("cli")
    );
    assert_eq!(
        bundle["provenance"]["sandbox_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["autocompact_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["autocompact_pct_source"].as_str(),
        Some("profile-harness-override")
    );
    assert_eq!(
        bundle["provenance"]["native_config_source"].as_str(),
        Some("profile-harness-override")
    );
}

#[test]
fn build_launch_bundle_invalid_native_config_shape_fails_with_diagnostic() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness-overrides:
  codex:
    native-config: [1, 2]
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
        "codex",
    ]);

    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("native-config"));
}

#[test]
fn build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
tools:
  bash: allow
  agent: deny
  edit: deny
disallowed-tools: [write]
mcp-tools: [plugin:root]
harness-overrides:
  codex:
    tools:
      "bash(meridian spawn *)": allow
      agent: deny
    disallowed-tools: [edit]
    mcp-tools: [plugin:override]
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut root_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    root_cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    let root_output = root_cmd.assert().success().get_output().clone();
    let root_bundle: Value = serde_json::from_slice(&root_output.stdout).unwrap();
    assert_eq!(root_bundle["tools"]["allowed"], serde_json::json!(["Bash"]));
    assert_eq!(
        root_bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit", "Write"])
    );
    assert_eq!(
        root_bundle["tools"]["mcp"],
        serde_json::json!(["plugin:root"])
    );

    let mut override_cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    override_cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);
    let override_output = override_cmd.assert().success().get_output().clone();
    let override_bundle: Value = serde_json::from_slice(&override_output.stdout).unwrap();
    assert_eq!(
        override_bundle["tools"]["allowed"],
        serde_json::json!(["Bash(meridian spawn *)"])
    );
    assert_eq!(
        override_bundle["tools"]["disallowed"],
        serde_json::json!(["Agent", "Edit"])
    );
    assert_eq!(
        override_bundle["tools"]["mcp"],
        serde_json::json!(["plugin:override"])
    );
}

#[test]
fn build_launch_bundle_profile_execution_policy_flows_without_cli_override() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
effort: xhigh
approval: auto
sandbox: workspace-write
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["execution_policy"]["effort"].as_str(), Some("xhigh"));
    assert_eq!(
        bundle["execution_policy"]["approval"].as_str(),
        Some("auto")
    );
    assert_eq!(
        bundle["execution_policy"]["sandbox"].as_str(),
        Some("workspace-write")
    );
    assert_eq!(
        bundle["provenance"]["effort_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["approval_source"].as_str(),
        Some("profile")
    );
    assert_eq!(
        bundle["provenance"]["sandbox_source"].as_str(),
        Some("profile")
    );
}

#[test]
fn build_launch_bundle_cli_direct_model_id_prefers_provider_harness_over_profile() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
harness: claude
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
        "--model",
        "gpt-5",
    ]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["model_token"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["model"].as_str(), Some("gpt-5"));
    assert_eq!(bundle["routing"]["harness"].as_str(), Some("codex"));
    assert_eq!(bundle["provenance"]["model_source"].as_str(), Some("cli"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("provider")
    );
}

#[test]
fn build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: unknown-model-token
---
Review code changes."#;

    let extra_toml = r#"[settings]
default_harness = "invalid-harness""#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(bundle["routing"]["harness"].as_str(), Some("claude"));
    assert_eq!(
        bundle["provenance"]["harness_source"].as_str(),
        Some("default")
    );
    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("settings.default_harness")
    }));
}

#[test]
fn build_launch_bundle_fails_when_no_model_available() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains("requires a model"));
}

#[test]
fn build_launch_bundle_fails_when_agent_file_missing() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", agent_content, &[], "");

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "missing-agent"]);
    cmd.assert()
        .failure()
        .stderr(predicates::str::contains("missing-agent"))
        .stderr(predicates::str::contains("read launch bundle agent"));
}

#[test]
fn build_launch_bundle_has_canonical_prompt_surface_for_small_fixture() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
description: Review implementation
mode: subagent
model: claude-opus-4-6
skills: [principle_a, reference_a]
---
Review code changes."#;
    let principle_skill =
        "---\nname: principle_a\ndescription: Principle\ntype: principle\n---\nPrinciple body.";
    let reference_skill =
        "---\nname: reference_a\ndescription: Reference\ntype: reference\n---\nReference body.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[
            ("principle_a", principle_skill),
            ("reference_a", reference_skill),
        ],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();
    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");

    let expected = concat!(
        "# Agent Profile\n\n",
        "Review code changes.\n\n",
        "# Skill: principle_a\n\n",
        "Principle body.\n\n",
        "# Skill: reference_a\n\n",
        "Reference body.\n\n",
        "# Meridian Agents\n\n",
        "Installed Meridian agents available at launch time.\n\n",
        "## Subagent\n",
        "- reviewer: Review implementation | Model: claude-opus-4-6\n\n",
        "# Report\n\n",
        "**IMPORTANT - Your final assistant message must be the run report.**\n\n",
        "Provide a plain markdown report in your final assistant message.\n\n",
        "Include: what was done, key decisions made, files created/modified, verification results, and any issues or blockers.\n\n",
        "# Skill: principle_a\n\n",
        "Principle body."
    );
    assert_eq!(system_instruction, expected);
}
