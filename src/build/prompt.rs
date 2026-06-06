use std::collections::HashSet;
use std::path::Path;

use crate::build::bundle::{AvailableSkill, LoadedSkill, SupplementalDoc};
use crate::build::inventory::build_inventory_prompt;
use crate::compiler::skills::parse_skill_content;
use crate::compiler::variants::harness_skill_variant_path;
use crate::error::MarsError;
use crate::frontmatter::{Frontmatter, SkillsSpec};

const REPORT_INSTRUCTION: &str = "# Report\n\n**IMPORTANT - Your final assistant message must be the run report.**\n\nProvide a plain markdown report in your final assistant message.\n\nInclude: what was done, key decisions made, files created/modified, verification results, and any issues or blockers.";

pub struct PromptCompilation {
    pub system_instruction: String,
    pub supplemental_documents: Vec<SupplementalDoc>,
    pub inventory_prompt: String,
    pub loaded_skills: Vec<LoadedSkill>,
    pub available_skills: Vec<AvailableSkill>,
    pub missing_skills: Vec<String>,
    pub warnings: Vec<String>,
}

struct LoadedSkillDocument {
    requested_index: usize,
    document: SupplementalDoc,
    /// Raw body without `# Skill: name` heading.
    body: String,
}

struct LoadedSkillData {
    document: SupplementalDoc,
    body: String,
}

enum SkillLoadOutcome {
    Loaded(LoadedSkillData),
    Missing,
}

#[derive(Debug, Clone)]
enum AvailableSkillOutcome {
    Available(AvailableSkill),
    Missing,
}

#[allow(clippy::too_many_arguments)]
pub fn compile_prompt_surface(
    mars_dir: &Path,
    agent_body: &str,
    profile_skills: &SkillsSpec,
    extra_skills: &[String],
    harness_id: &str,
    selected_model_token: &str,
    canonical_model_id: &str,
    subagents_filter: &[String],
) -> Result<PromptCompilation, MarsError> {
    let _ = (selected_model_token, canonical_model_id);

    let requested_load_skills = requested_skill_order(&profile_skills.load, extra_skills);
    let requested_available_skills = requested_available_skill_order(
        &profile_skills.available,
        requested_load_skills.iter().map(String::as_str),
    );

    let mut loaded_documents = Vec::new();
    let mut missing_skills = Vec::new();
    let mut warnings = Vec::new();

    for (requested_index, skill) in requested_load_skills.iter().enumerate() {
        match load_skill_document(mars_dir, skill, harness_id) {
            Ok(SkillLoadOutcome::Loaded(data)) => {
                loaded_documents.push(LoadedSkillDocument {
                    requested_index,
                    document: data.document,
                    body: data.body,
                });
            }
            Ok(SkillLoadOutcome::Missing) => missing_skills.push(skill.clone()),

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
        .map(|loaded| LoadedSkill {
            name: loaded.document.name.clone(),
            skill_type: loaded.document.skill_type.clone(),
            body: loaded.body.clone(),
        })
        .collect::<Vec<_>>();

    let mut available_skills = Vec::new();
    for skill in &requested_available_skills {
        match resolve_available_skill(mars_dir, skill, harness_id) {
            Ok(AvailableSkillOutcome::Available(skill)) => available_skills.push(skill),
            Ok(AvailableSkillOutcome::Missing) => missing_skills.push(skill.clone()),

            Err(err) => {
                warnings.push(err);
                missing_skills.push(skill.clone());
            }
        }
    }

    let inventory_prompt =
        build_inventory_prompt(mars_dir, subagents_filter, harness_id, &mut warnings)?;
    let system_instruction = compose_system_instruction(
        agent_body,
        &supplemental_documents,
        &available_skills,
        &inventory_prompt,
        REPORT_INSTRUCTION,
    );

    Ok(PromptCompilation {
        system_instruction,
        supplemental_documents,
        inventory_prompt,
        loaded_skills,
        available_skills,
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

fn requested_available_skill_order<'a>(
    available_skills: &[String],
    loaded_skills: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let mut blocked = loaded_skills
        .map(|name| name.trim().to_string())
        .collect::<HashSet<_>>();
    blocked.remove("");

    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for name in available_skills {
        let normalized = name.trim();
        if normalized.is_empty() || blocked.contains(normalized) {
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

    // model-invocable gates global discovery, not explicit profile references.
    // If the agent profile lists a skill, it loads regardless.
    let (_base_profile, base_frontmatter) = parse_skill_file(skill_name, &base_skill_path)?;

    let selected_skill_path =
        harness_skill_variant_path(&skill_dir, harness_id).unwrap_or(base_skill_path);
    let (_, selected_frontmatter) = parse_skill_file(skill_name, &selected_skill_path)?;

    let skill_type = skill_type_from_frontmatter(&selected_frontmatter)
        .or_else(|| skill_type_from_frontmatter(&base_frontmatter));
    let skill_type = skill_type.unwrap_or_else(|| "reference".to_string());

    let body = selected_frontmatter.body().trim().to_string();
    let content = render_skill_content_block(skill_name, &body);

    Ok(SkillLoadOutcome::Loaded(LoadedSkillData {
        document: SupplementalDoc {
            kind: "skill".to_string(),
            name: skill_name.to_string(),
            content,
            skill_type,
        },
        body,
    }))
}

fn resolve_available_skill(
    mars_dir: &Path,
    skill_name: &str,
    harness_id: &str,
) -> Result<AvailableSkillOutcome, String> {
    let skill_dir = mars_dir.join("skills").join(skill_name);
    let base_skill_path = skill_dir.join("SKILL.md");
    if !base_skill_path.is_file() {
        return Ok(AvailableSkillOutcome::Missing);
    }

    // model-invocable gates global discovery, not explicit profile references.
    let (base_profile, base_frontmatter) = parse_skill_file(skill_name, &base_skill_path)?;

    let selected_skill_path =
        harness_skill_variant_path(&skill_dir, harness_id).unwrap_or(base_skill_path);
    let (selected_profile, selected_frontmatter) =
        parse_skill_file(skill_name, &selected_skill_path)?;

    let skill_type = skill_type_from_frontmatter(&selected_frontmatter)
        .or_else(|| skill_type_from_frontmatter(&base_frontmatter))
        .unwrap_or_else(|| "reference".to_string());
    let description = selected_profile
        .description
        .or(base_profile.description)
        .unwrap_or_default();

    Ok(AvailableSkillOutcome::Available(AvailableSkill {
        name: skill_name.to_string(),
        skill_type,
        description,
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
    available_skills: &[AvailableSkill],
    inventory_prompt: &str,
    report_instruction: &str,
) -> String {
    let mut blocks: Vec<String> = Vec::new();

    let body = agent_body.trim();
    if !body.is_empty() {
        blocks.push(format!("# Agent Profile\n\n{body}"));
    }

    // Auto-loaded skills: full content, already sorted by type priority
    // (principles first, then guardrails, then others).
    // Each skill has its own `# Skill: name` heading — no intermediate
    // wrapper headings that would break markdown hierarchy.
    for doc in supplemental_documents {
        let content = doc.content.trim();
        if !content.is_empty() {
            blocks.push(content.to_string());
        }
    }

    // Available skills: names only, grouped by type.
    // NOTE: meridian-cli recomposes this block independently in
    // `composition.py::_render_available_skills_block`. Keep format in sync.
    if !available_skills.is_empty() {
        let mut avail_block = String::from(
            "# Available Skills\n\nNot yet loaded. Load proactively when the task fits.",
        );
        for (type_label, type_key, description) in &[
            (
                "Principles",
                "principle",
                "Override other guidance when loaded.",
            ),
            (
                "Guardrails",
                "guardrail",
                "Load before acting in sensitive areas.",
            ),
            (
                "Mode-shift",
                "mode-shift",
                "Change how you operate when loaded.",
            ),
            (
                "Checkpoint",
                "checkpoint",
                "Load at decision points to verify before continuing.",
            ),
        ] {
            let skills: Vec<_> = available_skills
                .iter()
                .filter(|s| s.skill_type == *type_key)
                .collect();
            if !skills.is_empty() {
                avail_block.push_str(&format!("\n\n## {type_label}\n{description}"));
                for skill in skills {
                    avail_block.push_str(&format!("\n- {}", skill.name));
                }
            }
        }
        // Remaining types: each gets its own heading, no description.
        let other_skills: Vec<_> = available_skills
            .iter()
            .filter(|s| {
                s.skill_type != "principle"
                    && s.skill_type != "guardrail"
                    && s.skill_type != "mode-shift"
                    && s.skill_type != "checkpoint"
            })
            .collect();
        if !other_skills.is_empty() {
            let mut seen_types: Vec<&str> = Vec::new();
            for s in &other_skills {
                if !seen_types.contains(&s.skill_type.as_str()) {
                    seen_types.push(&s.skill_type);
                }
            }
            for type_key in &seen_types {
                let group: Vec<_> = other_skills
                    .iter()
                    .filter(|s| s.skill_type == *type_key)
                    .collect();
                let mut capitalized = type_key.to_string();
                if let Some(first) = capitalized.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                avail_block.push_str(&format!("\n\n## {capitalized}"));
                for skill in group {
                    avail_block.push_str(&format!("\n- {}", skill.name));
                }
            }
        }
        blocks.push(avail_block);
    }

    let inventory = inventory_prompt.trim();
    if !inventory.is_empty() {
        blocks.push(inventory.to_string());
    }

    blocks.push(report_instruction.to_string());

    blocks.join("\n\n")
}
