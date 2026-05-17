use std::collections::HashSet;
use std::path::Path;

use crate::build::bundle::SupplementalDoc;
use crate::compiler::skills::parse_skill_content;

const REPORT_INSTRUCTION: &str = "When complete, return a plain markdown report with what was done, key decisions, files modified, verification results, and blockers.";

pub struct PromptCompilation {
    pub system_instruction: String,
    pub supplemental_documents: Vec<SupplementalDoc>,
    pub inventory_prompt: String,
    pub loaded_skills: Vec<String>,
    pub missing_skills: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn compile_prompt_surface(
    mars_dir: &Path,
    agent_body: &str,
    profile_skills: &[String],
    extra_skills: &[String],
) -> PromptCompilation {
    let requested_skills = requested_skill_order(profile_skills, extra_skills);

    let mut supplemental_documents = Vec::new();
    let mut loaded_skills = Vec::new();
    let mut missing_skills = Vec::new();
    let mut warnings = Vec::new();

    for skill in &requested_skills {
        match load_skill_document(mars_dir, skill) {
            Ok(Some(document)) => {
                supplemental_documents.push(document);
                loaded_skills.push(skill.clone());
            }
            Ok(None) => missing_skills.push(skill.clone()),
            Err(err) => {
                warnings.push(err);
                missing_skills.push(skill.clone());
            }
        }
    }

    let inventory_prompt = String::new();
    let system_instruction = compose_system_instruction(
        agent_body,
        &supplemental_documents,
        REPORT_INSTRUCTION,
        &inventory_prompt,
    );

    PromptCompilation {
        system_instruction,
        supplemental_documents,
        inventory_prompt,
        loaded_skills,
        missing_skills,
        warnings,
    }
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

fn load_skill_document(
    mars_dir: &Path,
    skill_name: &str,
) -> Result<Option<SupplementalDoc>, String> {
    let skill_path = mars_dir.join("skills").join(skill_name).join("SKILL.md");
    if !skill_path.is_file() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&skill_path).map_err(|err| {
        format!(
            "failed to read skill `{skill_name}` from {}: {err}",
            skill_path.display()
        )
    })?;

    let mut skill_diags = Vec::new();
    let content = match parse_skill_content(&raw, &mut skill_diags) {
        Ok((_profile, frontmatter)) => frontmatter.body().trim().to_string(),
        Err(err) => {
            return Err(format!(
                "failed to parse skill `{skill_name}` from {}: {err}",
                skill_path.display()
            ));
        }
    };

    if let Some(diag) = skill_diags.first() {
        return Err(format!(
            "skill `{skill_name}` has invalid frontmatter in {}: {}",
            skill_path.display(),
            diag.message()
        ));
    }

    Ok(Some(SupplementalDoc {
        kind: "skill".to_string(),
        name: skill_name.to_string(),
        content,
        skill_type: "reference".to_string(),
    }))
}

fn compose_system_instruction(
    agent_body: &str,
    supplemental_documents: &[SupplementalDoc],
    report_instruction: &str,
    inventory_prompt: &str,
) -> String {
    let mut blocks: Vec<String> = Vec::new();

    let body = agent_body.trim();
    if !body.is_empty() {
        blocks.push(format!("# Agent Profile\n\n{body}"));
    }

    for doc in supplemental_documents {
        let content = doc.content.trim();
        if content.is_empty() {
            continue;
        }
        blocks.push(format!("# Skill: {}\n\n{}", doc.name, content));
    }

    blocks.push(format!("# Report Contract\n\n{report_instruction}"));

    let inventory = inventory_prompt.trim();
    if !inventory.is_empty() {
        blocks.push(format!("# Inventory\n\n{inventory}"));
    }

    blocks.join("\n\n")
}
