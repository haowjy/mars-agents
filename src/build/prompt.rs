use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::build::bundle::SupplementalDoc;
use crate::compiler::agents::{AgentMode, ModelPolicyMatchType, parse_agent_content};
use crate::compiler::skills::parse_skill_content;
use crate::compiler::variants::harness_skill_variant_path;
use crate::error::{ConfigError, MarsError};
use crate::frontmatter::Frontmatter;

const REPORT_INSTRUCTION: &str = "# Report\n\n**IMPORTANT - Your final assistant message must be the run report.**\n\nProvide a plain markdown report in your final assistant message.\n\nInclude: what was done, key decisions made, files created/modified, verification results, and any issues or blockers.";

pub struct PromptCompilation {
    pub system_instruction: String,
    pub supplemental_documents: Vec<SupplementalDoc>,
    pub inventory_prompt: String,
    pub loaded_skills: Vec<String>,
    pub missing_skills: Vec<String>,
    pub warnings: Vec<String>,
}

struct LoadedSkillDocument {
    requested_index: usize,
    document: SupplementalDoc,
}

enum SkillLoadOutcome {
    Loaded(SupplementalDoc),
    Missing,
    SkippedModelInvocableFalse,
}

#[derive(Debug, Clone)]
struct ParsedAgentInventory {
    name: String,
    description: String,
    model: Option<String>,
    fanout: Vec<String>,
    mode: AgentMode,
}

pub fn compile_prompt_surface(
    mars_dir: &Path,
    agent_body: &str,
    profile_skills: &[String],
    extra_skills: &[String],
    harness_id: &str,
    selected_model_token: &str,
    canonical_model_id: &str,
) -> Result<PromptCompilation, MarsError> {
    let _ = (selected_model_token, canonical_model_id);

    let requested_skills = requested_skill_order(profile_skills, extra_skills);

    let mut loaded_documents = Vec::new();
    let mut missing_skills = Vec::new();
    let mut warnings = Vec::new();

    for (requested_index, skill) in requested_skills.iter().enumerate() {
        match load_skill_document(mars_dir, skill, harness_id) {
            Ok(SkillLoadOutcome::Loaded(document)) => {
                loaded_documents.push(LoadedSkillDocument {
                    requested_index,
                    document,
                });
            }
            Ok(SkillLoadOutcome::Missing) => missing_skills.push(skill.clone()),
            Ok(SkillLoadOutcome::SkippedModelInvocableFalse) => {}
            Err(err) => {
                warnings.push(err);
                missing_skills.push(skill.clone());
            }
        }
    }

    loaded_documents.sort_by(|left, right| {
        let left_key = (
            skill_type_priority(&left.document.skill_type),
            left.requested_index,
        );
        let right_key = (
            skill_type_priority(&right.document.skill_type),
            right.requested_index,
        );
        left_key.cmp(&right_key)
    });

    let supplemental_documents = loaded_documents
        .iter()
        .map(|loaded| loaded.document.clone())
        .collect::<Vec<_>>();

    let loaded_skills = loaded_documents
        .iter()
        .map(|loaded| loaded.document.name.clone())
        .collect::<Vec<_>>();

    let inventory_prompt = build_inventory_prompt(mars_dir, &mut warnings)?;
    let system_instruction = compose_system_instruction(
        agent_body,
        &supplemental_documents,
        &inventory_prompt,
        REPORT_INSTRUCTION,
    );

    Ok(PromptCompilation {
        system_instruction,
        supplemental_documents,
        inventory_prompt,
        loaded_skills,
        missing_skills,
        warnings,
    })
}

fn requested_skill_order(profile_skills: &[String], extra_skills: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    for name in profile_skills.iter().chain(extra_skills.iter()) {
        let normalized = name.trim();
        if normalized.is_empty() {
            continue;
        }
        if seen.insert(normalized.to_string()) {
            ordered.push(normalized.to_string());
        }
    }

    ordered
}

fn skill_type_priority(skill_type: &str) -> u8 {
    match skill_type {
        "principle" => 0,
        "guardrail" => 1,
        "reference" => 2,
        _ => 2,
    }
}

fn load_skill_document(
    mars_dir: &Path,
    skill_name: &str,
    harness_id: &str,
) -> Result<SkillLoadOutcome, String> {
    let skill_dir = mars_dir.join("skills").join(skill_name);
    let base_skill_path = skill_dir.join("SKILL.md");
    if !base_skill_path.is_file() {
        return Ok(SkillLoadOutcome::Missing);
    }

    let (base_profile, base_frontmatter) = parse_skill_file(skill_name, &base_skill_path)?;
    if !base_profile.model_invocable {
        return Ok(SkillLoadOutcome::SkippedModelInvocableFalse);
    }

    let selected_skill_path =
        harness_skill_variant_path(&skill_dir, harness_id).unwrap_or(base_skill_path);
    let (_, selected_frontmatter) = parse_skill_file(skill_name, &selected_skill_path)?;

    let skill_type = skill_type_from_frontmatter(&selected_frontmatter)
        .or_else(|| skill_type_from_frontmatter(&base_frontmatter));
    let skill_type = skill_type.unwrap_or_else(|| "reference".to_string());

    let content = render_skill_content_block(skill_name, selected_frontmatter.body().trim());

    Ok(SkillLoadOutcome::Loaded(SupplementalDoc {
        kind: "skill".to_string(),
        name: skill_name.to_string(),
        content,
        skill_type,
    }))
}

fn parse_skill_file(
    skill_name: &str,
    skill_path: &Path,
) -> Result<(crate::compiler::skills::SkillProfile, Frontmatter), String> {
    let raw = std::fs::read_to_string(skill_path).map_err(|err| {
        format!(
            "failed to read skill `{skill_name}` from {}: {err}",
            skill_path.display()
        )
    })?;

    let mut skill_diags = Vec::new();
    let parsed = parse_skill_content(&raw, &mut skill_diags).map_err(|err| {
        format!(
            "failed to parse skill `{skill_name}` from {}: {err}",
            skill_path.display()
        )
    })?;

    if let Some(diag) = skill_diags.first() {
        return Err(format!(
            "skill `{skill_name}` has invalid frontmatter in {}: {}",
            skill_path.display(),
            diag.message()
        ));
    }

    Ok(parsed)
}

fn skill_type_from_frontmatter(frontmatter: &Frontmatter) -> Option<String> {
    frontmatter
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn render_skill_content_block(skill_name: &str, body: &str) -> String {
    if body.is_empty() {
        format!("# Skill: {skill_name}")
    } else {
        format!("# Skill: {skill_name}\n\n{body}")
    }
}

fn compose_system_instruction(
    agent_body: &str,
    supplemental_documents: &[SupplementalDoc],
    inventory_prompt: &str,
    report_instruction: &str,
) -> String {
    let mut blocks: Vec<String> = Vec::new();

    let body = agent_body.trim();
    if !body.is_empty() {
        blocks.push(format!("# Agent Profile\n\n{body}"));
    }

    for doc in supplemental_documents {
        let content = doc.content.trim();
        if !content.is_empty() {
            blocks.push(content.to_string());
        }
    }

    let inventory = inventory_prompt.trim();
    if !inventory.is_empty() {
        blocks.push(inventory.to_string());
    }

    blocks.push(report_instruction.to_string());

    for doc in supplemental_documents
        .iter()
        .filter(|doc| doc.skill_type == "principle")
    {
        let content = doc.content.trim();
        if !content.is_empty() {
            blocks.push(content.to_string());
        }
    }

    blocks.join("\n\n")
}

fn build_inventory_prompt(
    mars_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<String, MarsError> {
    let agents_dir = mars_dir.join("agents");
    if !agents_dir.is_dir() {
        return Ok(String::new());
    }

    let read_dir = match std::fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!(
                "failed to read agent inventory from {}: {err}",
                agents_dir.display()
            ));
            return Ok(String::new());
        }
    };

    let mut agent_paths: Vec<PathBuf> = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    agent_paths.sort();

    let mut primary_agents = Vec::new();
    let mut subagent_agents = Vec::new();

    for path in agent_paths {
        match parse_inventory_agent(&path) {
            Ok((Some(agent), agent_warnings)) => {
                warnings.extend(agent_warnings);
                if agent.mode == AgentMode::Primary {
                    primary_agents.push(agent);
                } else {
                    subagent_agents.push(agent);
                }
            }
            Ok((None, agent_warnings)) => warnings.extend(agent_warnings),
            Err(err) => {
                return Err(MarsError::Config(ConfigError::Invalid { message: err }));
            }
        }
    }

    if primary_agents.is_empty() && subagent_agents.is_empty() {
        return Ok(String::new());
    }

    primary_agents.sort_by(|left, right| left.name.cmp(&right.name));
    subagent_agents.sort_by(|left, right| left.name.cmp(&right.name));

    let mut lines = vec![
        "# Meridian Agents".to_string(),
        "".to_string(),
        "Installed Meridian agents available at launch time.".to_string(),
    ];

    if !primary_agents.is_empty() {
        lines.extend(["".to_string(), "## Primary".to_string()]);
        for agent in &primary_agents {
            lines.push(render_inventory_line(agent));
        }
    }

    if !subagent_agents.is_empty() {
        lines.extend(["".to_string(), "## Subagent".to_string()]);
        for agent in &subagent_agents {
            lines.push(render_inventory_line(agent));
        }
    }

    Ok(lines.join("\n").trim().to_string())
}

fn parse_inventory_agent(
    path: &Path,
) -> Result<(Option<ParsedAgentInventory>, Vec<String>), String> {
    let content = std::fs::read_to_string(path).map_err(|err| {
        format!(
            "failed to read agent inventory file {}: {err}",
            path.display()
        )
    })?;

    let mut parse_diags = Vec::new();
    let (profile, _frontmatter) =
        parse_agent_content(&content, &mut parse_diags).map_err(|err| {
            format!(
                "failed to parse agent inventory file {}: {err}",
                path.display()
            )
        })?;

    let mut warnings = Vec::new();
    for diag in parse_diags {
        if diag.is_error() {
            return Err(format!(
                "agent inventory file {} has invalid frontmatter: {}",
                path.display(),
                diag.message()
            ));
        }
        warnings.push(format!(
            "agent inventory parse warning in {}: {}",
            path.display(),
            diag.message()
        ));
    }
    if !profile.model_invocable {
        return Ok((None, warnings));
    }

    let fallback_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-agent")
        .to_string();
    let fanout = fallback_model_policies_for_inventory(&profile);
    let name = profile.name.unwrap_or(fallback_name);
    let description = profile.description.unwrap_or_default();
    let mode = profile.mode.clone().unwrap_or(AgentMode::Subagent);

    Ok((
        Some(ParsedAgentInventory {
            name,
            description,
            model: profile.model,
            fanout,
            mode,
        }),
        warnings,
    ))
}

fn fallback_model_policies_for_inventory(
    profile: &crate::compiler::agents::AgentProfile,
) -> Vec<String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    // Limitation: this deduplicates exact fallback labels only. Alias-to-model
    // canonical dedupe requires alias catalog context not currently loaded here.
    for policy in &profile.model_policies {
        if policy.no_fallback {
            continue;
        }
        if !matches!(
            policy.match_type,
            ModelPolicyMatchType::Alias | ModelPolicyMatchType::Model
        ) {
            continue;
        }
        let value = policy.match_value.trim();
        if value.is_empty() {
            continue;
        }
        if seen.insert(value.to_string()) {
            entries.push(value.to_string());
        }
    }

    entries
}

fn render_inventory_line(agent: &ParsedAgentInventory) -> String {
    let description = agent.description.trim();
    let mut line = if description.is_empty() {
        format!("- {}", agent.name)
    } else {
        format!("- {}: {}", agent.name, description)
    };

    if let Some(model) = agent.model.as_ref().map(|value| value.trim())
        && !model.is_empty()
    {
        line.push_str(" | Model: ");
        line.push_str(model);
    }

    if !agent.fanout.is_empty() {
        line.push_str(" | Fan-out: ");
        line.push_str(&agent.fanout.join(", "));
    }

    line
}
