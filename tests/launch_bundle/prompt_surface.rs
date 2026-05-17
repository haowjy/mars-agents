use super::common::{setup_bundle_project, setup_bundle_project_with_agents};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_includes_skill_documents_and_system_instruction() {
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

pub(crate) fn build_launch_bundle_uses_harness_variant_skill_for_codex() {
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

pub(crate) fn build_launch_bundle_uses_harness_override_skills_for_prompt_surface() {
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

pub(crate) fn build_launch_bundle_skips_model_non_invocable_skills() {
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

pub(crate) fn build_launch_bundle_includes_inventory_prompt_before_report_block() {
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

pub(crate) fn build_launch_bundle_orders_skills_by_type_and_bookends_principles() {
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

pub(crate) fn build_launch_bundle_inventory_hides_model_non_invocable_agents_and_shows_fanout() {
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

pub(crate) fn build_launch_bundle_merges_extra_skills_after_profile_dedupes_and_tracks_missing() {
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

pub(crate) fn build_launch_bundle_has_canonical_prompt_surface_for_small_fixture() {
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
