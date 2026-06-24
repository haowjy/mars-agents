use super::common::{
    install_fake_harnesses, replace_path_with, setup_bundle_project,
    setup_bundle_project_with_agents,
};
use crate::test_common::{API_PATH, mars_cmd};
use assert_fs::TempDir;
use serde_json::Value;

pub(crate) fn build_launch_bundle_includes_skill_documents_and_system_instruction() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: gpt-5
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
        bundle["skills"]["loaded"][0]["name"].as_str(),
        Some("planning")
    );
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));
}

pub(crate) fn build_launch_bundle_keeps_skill_with_snake_case_tool_alias() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: gpt-5
skills: [planning]
---
Review code changes."#;
    let skill_content =
        "---\nname: planning\ndescription: Plan tasks\ntools: [ask_user]\n---\nUse this skill.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[("planning", skill_content)],
        "",
    );

    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args([
        "build",
        "launch-bundle",
        "--agent",
        "reviewer",
        "--harness",
        "codex",
    ]);
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        bundle["skills"]["loaded"][0]["name"].as_str(),
        Some("planning")
    );
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));
    let warnings = bundle["warnings"].as_array().unwrap();
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
}

pub(crate) fn build_launch_bundle_splits_loaded_and_available_skills() {
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills:
  load: [dev_principles]
  available: [planning, workspace]
---
Review code changes."#;
    let dev_principles = "---\nname: dev_principles\ndescription: Core principles\ntype: principle\n---\nAlways be precise.";
    let planning = "---\nname: planning\ndescription: Use when decomposing phased work.\ntype: reference\n---\nPlanning body should stay out of prompt.";
    let workspace = "---\nname: workspace\ndescription: Use when other agents may have changes.\ntype: guardrail\n---\nWorkspace body should stay out of prompt.";

    let (server, project_root) = setup_bundle_project(
        &temp,
        "bundle-source",
        agent_content,
        &[
            ("dev_principles", dev_principles),
            ("planning", planning),
            ("workspace", workspace),
        ],
        "",
    );

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let loaded = bundle["skills"]["loaded"].as_array().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["name"].as_str(), Some("dev_principles"));
    assert_eq!(loaded[0]["skill_type"].as_str(), Some("principle"));
    assert!(
        loaded[0]["body"]
            .as_str()
            .unwrap()
            .contains("Always be precise.")
    );

    let available = bundle["skills"]["available"].as_array().unwrap();
    assert_eq!(available.len(), 2);
    assert_eq!(available[0]["name"].as_str(), Some("planning"));
    assert_eq!(available[0]["skill_type"].as_str(), Some("reference"));
    assert_eq!(
        available[0]["description"].as_str(),
        Some("Use when decomposing phased work.")
    );
    assert_eq!(available[1]["name"].as_str(), Some("workspace"));
    assert_eq!(available[1]["skill_type"].as_str(), Some("guardrail"));
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("dev_principles"));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: dev_principles"));
    assert!(!system_instruction.contains("Planning body should stay out of prompt."));
    assert!(!system_instruction.contains("Workspace body should stay out of prompt."));
}

pub(crate) fn build_launch_bundle_uses_harness_variant_skill_for_codex() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
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
        "---\nname: planning\ndescription: Variant metadata ignored\ntype: reference\ntools: [askuser]\n---\nCodex variant content.",
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
    cmd.env("PATH", replace_path_with(&bin_dir));

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
    let warnings = bundle["warnings"].as_array().unwrap();
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.as_str().unwrap_or_default().contains("askuser"))
    );
}

pub(crate) fn build_launch_bundle_harness_override_skills_are_passthrough_for_prompt_surface() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["codex"]);
    let agent_content = r#"---
name: reviewer
model: gpt-5
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
    cmd.env("PATH", replace_path_with(&bin_dir));

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let docs = bundle["prompt_surface"]["supplemental_documents"]
        .as_array()
        .expect("supplemental_documents should be an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["name"].as_str(), Some("planning"));
    assert!(
        docs[0]["content"]
            .as_str()
            .unwrap()
            .contains("Planning base content.")
    );

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("# Skill: planning"));
    assert!(!system_instruction.contains("# Skill: codex_skill"));

    assert_eq!(
        bundle["skills"]["loaded"][0]["name"].as_str(),
        Some("planning")
    );
    assert_eq!(
        bundle["execution_policy"]["native_config"]["skills"],
        serde_json::json!(["codex_skill"])
    );
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));
}

pub(crate) fn build_launch_bundle_loads_model_non_invocable_skills_when_explicit() {
    // model-invocable gates global discovery, not explicit profile references.
    // If the agent profile explicitly lists a skill, it loads regardless.
    let temp = TempDir::new().unwrap();
    let agent_content = r#"---
name: reviewer
model: claude-opus-4-6
skills: [planning, hidden]
---
Review code changes."#;
    let planning_skill = "---\nname: planning\ndescription: Plan tasks\ntype: reference\n---\nVisible skill content.";
    let hidden_skill = "---\nname: hidden\ndescription: Hidden skill\nmodel-invocable: false\ntype: principle\n---\nExplicitly referenced content.";

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
    assert_eq!(docs.len(), 2);

    let loaded = bundle["skills"]["loaded"]
        .as_array()
        .expect("loaded should be an array");
    assert_eq!(loaded.len(), 2);
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(bundle["skills"]["missing"], serde_json::json!([]));

    let system_instruction = bundle["prompt_surface"]["system_instruction"]
        .as_str()
        .expect("system instruction should be string");
    assert!(system_instruction.contains("Explicitly referenced content."));
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
    assert!(
        inventory_prompt
            .contains("- `meridian spawn -a planner`: Plan tasks | Model: openai/gpt-5")
    );
    assert!(inventory_prompt.contains(
        "- `meridian spawn -a reviewer`: Review implementation | Model: claude-opus-4-6"
    ));

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

    // Skills are sorted by type priority (principle first, then guardrail, then reference)
    // but without wrapper headings — each skill keeps its own `# Skill:` heading.
    let principle_index = system_instruction
        .find("# Skill: principle_a")
        .expect("system instruction should include principle skill");
    let guardrail_index = system_instruction
        .find("# Skill: guardrail_a")
        .expect("system instruction should include guardrail skill");
    let reference_a_index = system_instruction
        .find("# Skill: reference_a")
        .expect("system instruction should include reference skill");
    let report_index = system_instruction
        .find("# Report")
        .expect("system instruction should include report block");

    // Order: principle -> guardrail -> reference -> report
    assert!(principle_index < guardrail_index);
    assert!(guardrail_index < reference_a_index);
    assert!(reference_a_index < report_index);

    // No principle bookend after report
    let after_report = &system_instruction[(report_index + "# Report".len())..];
    assert!(
        !after_report.contains("Principle body."),
        "principle bookend should no longer appear after report"
    );
}

pub(crate) fn build_launch_bundle_fanout_agent_dual_lists_in_inventory() {
    let temp = TempDir::new().unwrap();
    let bin_dir = install_fake_harnesses(temp.path(), &["claude"]);
    let reviewer_content = r#"---
name: reviewer
description: Review implementation
mode: subagent
model: claude-opus-4-6
---
Review code changes."#;

    let extra_toml = r#"
[settings]
targets = [".claude"]
agent_emission = "always"

[settings.meridian.fanout]
agents = ["reviewer"]
"#;

    let (server, project_root) = setup_bundle_project_with_agents(
        &temp,
        "bundle-source",
        &[("reviewer", reviewer_content)],
        &[],
        extra_toml,
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

    let inventory_prompt = bundle["prompt_surface"]["inventory_prompt"]
        .as_str()
        .expect("inventory_prompt should be string");
    assert!(inventory_prompt.contains("## Subagent"));
    assert!(inventory_prompt.contains(
        "- `meridian spawn -a reviewer`: Review implementation | Model: claude-opus-4-6"
    ));
    assert!(
        inventory_prompt.contains("## Claude Agents (use `Agent({subagent_type: \"...\"})` tool)")
    );
    assert!(inventory_prompt.contains("- reviewer: Review implementation"));
}

pub(crate) fn build_launch_bundle_warns_on_deprecated_agent_copy_fanout_agents() {
    let temp = TempDir::new().unwrap();
    let reviewer_content = r#"---
name: reviewer
model: claude-opus-4-6
---
Review code changes."#;

    let extra_toml = r#"
[settings.meridian.agent_copy]
fanout_agents = ["reviewer"]
"#;

    let (server, project_root) =
        setup_bundle_project(&temp, "bundle-source", reviewer_content, &[], extra_toml);

    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["build", "launch-bundle", "--agent", "reviewer"]);

    let output = cmd.assert().success().get_output().clone();
    let bundle: Value = serde_json::from_slice(&output.stdout).unwrap();

    let warnings = bundle["warnings"]
        .as_array()
        .expect("warnings should be an array");
    assert!(warnings.iter().any(|warning| {
        warning
            .as_str()
            .unwrap_or_default()
            .contains("[settings.meridian.fanout].agents")
    }));
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
    assert!(inventory_prompt.contains("`meridian spawn -a reviewer`: Review implementation"));
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

    let loaded_names = bundle["skills"]["loaded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(loaded_names, vec!["planning", "extra_skill"]);
    assert_eq!(bundle["skills"]["available"], serde_json::json!([]));
    assert_eq!(
        bundle["skills"]["missing"],
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

    // Assert structure (headings, spawn format, report contract), not prose copy.
    assert!(system_instruction.contains("# Agent Profile"));
    assert!(system_instruction.contains("Review code changes."));
    assert!(system_instruction.contains("# Skill: principle_a"));
    assert!(system_instruction.contains("Principle body."));
    assert!(system_instruction.contains("# Skill: reference_a"));
    assert!(system_instruction.contains("Reference body."));
    assert!(system_instruction.contains("# Meridian Agents"));
    assert!(system_instruction.contains("## Subagent"));
    assert!(system_instruction.contains(
        "- `meridian spawn -a reviewer`: Review implementation | Model: claude-opus-4-6"
    ));
    assert!(system_instruction.contains("# Report"));
    assert!(
        system_instruction
            .contains("**IMPORTANT - Your final assistant message must be the run report.**")
    );

    let profile_index = system_instruction
        .find("# Agent Profile")
        .expect("profile section");
    let principle_index = system_instruction
        .find("# Skill: principle_a")
        .expect("principle skill");
    let reference_index = system_instruction
        .find("# Skill: reference_a")
        .expect("reference skill");
    let inventory_index = system_instruction
        .find("# Meridian Agents")
        .expect("inventory section");
    let report_index = system_instruction.find("# Report").expect("report section");
    assert!(profile_index < principle_index);
    assert!(principle_index < reference_index);
    assert!(reference_index < inventory_index);
    assert!(inventory_index < report_index);
}
