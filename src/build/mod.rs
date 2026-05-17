pub mod bundle;
pub mod policy;
pub mod prompt;

use std::path::PathBuf;

use bundle::{LaunchBundle, ScaffoldSlots, SkillsMetadata, ToolsSpec};
use policy::{PolicyInput, resolve_policy};
use prompt::compile_prompt_surface;

use crate::cli::MarsContext;
use crate::compiler::agents::parse_agent_content;
use crate::error::{ConfigError, MarsError};

pub struct LaunchBundleRequest {
    pub agent: String,
    pub model: Option<String>,
    pub harness: Option<String>,
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub extra_skills: Vec<String>,
}

pub fn build_launch_bundle(
    ctx: &MarsContext,
    request: LaunchBundleRequest,
) -> Result<LaunchBundle, MarsError> {
    let agent_path = agent_file_path(&ctx.project_root, &request.agent);
    let agent_content = std::fs::read_to_string(&agent_path).map_err(|source| MarsError::Io {
        operation: "read launch bundle agent".to_string(),
        path: agent_path.clone(),
        source,
    })?;

    let mut parse_diags = Vec::new();
    let (profile, frontmatter) =
        parse_agent_content(&agent_content, &mut parse_diags).map_err(|err| {
            MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "failed to parse agent `{}` from {}: {err}",
                    request.agent,
                    agent_path.display()
                ),
            })
        })?;

    let mut warnings: Vec<String> = parse_diags
        .iter()
        .map(|diag| format!("agent `{}`: {}", request.agent, diag.message()))
        .collect();

    let policy = resolve_policy(PolicyInput {
        project_root: &ctx.project_root,
        profile: &profile,
        model_override: request.model.as_deref(),
        harness_override: request.harness.as_deref(),
        effort_override: request.effort.as_deref(),
        approval_override: request.approval.as_deref(),
        sandbox_override: request.sandbox.as_deref(),
    })?;

    warnings.extend(policy.warnings);

    let mars_dir = ctx.project_root.join(".mars");
    let prompt = compile_prompt_surface(
        &mars_dir,
        frontmatter.body(),
        &profile.skills,
        &request.extra_skills,
        &policy.routing.harness,
        &policy.routing.model_token,
        &policy.routing.model,
    );

    warnings.extend(prompt.warnings);

    Ok(LaunchBundle {
        version: 1,
        agent: request.agent,
        routing: policy.routing,
        execution_policy: policy.execution_policy,
        prompt_surface: bundle::PromptSurface {
            system_instruction: prompt.system_instruction,
            supplemental_documents: prompt.supplemental_documents,
            inventory_prompt: prompt.inventory_prompt,
        },
        scaffold_slots: ScaffoldSlots::placeholders(),
        tools: ToolsSpec {
            allowed: profile.tools,
            disallowed: profile.disallowed_tools,
            mcp: profile.mcp_tools,
        },
        skills_metadata: SkillsMetadata {
            loaded: prompt.loaded_skills,
            missing: prompt.missing_skills,
        },
        provenance: policy.provenance,
        warnings,
    })
}

fn agent_file_path(project_root: &std::path::Path, agent: &str) -> PathBuf {
    project_root
        .join(".mars")
        .join("agents")
        .join(format!("{agent}.md"))
}
