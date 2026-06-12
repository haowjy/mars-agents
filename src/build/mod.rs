pub mod bundle;
pub mod inventory;
pub mod policy;
pub mod prompt;
pub mod tool_normalize;

use std::path::PathBuf;

use bundle::{LaunchBundle, ScaffoldSlots, Skills, ToolsSpec};
use policy::{PolicyInput, resolve_policy};
use prompt::compile_prompt_surface;
use tool_normalize::{ToolProjectionStatus, is_first_class_harness, normalize_tool_for_harness};

use crate::cli::MarsContext;
use crate::compiler::agents::{AgentProfile, HarnessKind, parse_agent_content};
use crate::config::EffectiveProjectConfig;
use crate::error::{ConfigError, MarsError};
use crate::frontmatter::SkillsSpec;

pub const LAUNCH_BUNDLE_VERSION: u32 = 3;

pub struct LaunchBundleRequest {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub harness: Option<String>,
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub extra_skills: Vec<String>,
    pub models_refresh: crate::models::ModelsRefreshControl,
}

pub fn build_launch_bundle(
    ctx: &MarsContext,
    request: LaunchBundleRequest,
) -> Result<LaunchBundle, MarsError> {
    let mut warnings: Vec<String> = Vec::new();
    let profile: AgentProfile;
    let agent_body: Option<String>;

    if let Some(agent) = request.agent.as_deref() {
        let agent_path = agent_file_path(&ctx.project_root, agent);
        let agent_content =
            std::fs::read_to_string(&agent_path).map_err(|source| MarsError::Io {
                operation: "read launch bundle agent".to_string(),
                path: agent_path.clone(),
                source,
            })?;

        let mut parse_diags = Vec::new();
        let (parsed_profile, frontmatter) = parse_agent_content(&agent_content, &mut parse_diags)
            .map_err(|err| {
            MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "failed to parse agent `{agent}` from {}: {err}",
                    agent_path.display()
                ),
            })
        })?;

        if let Some(fatal) = parse_diags.iter().find(|diag| diag.is_error()) {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "agent `{agent}` has invalid frontmatter in {}: {}",
                    agent_path.display(),
                    fatal.message()
                ),
            }));
        }

        warnings.extend(
            parse_diags
                .iter()
                .map(|diag| format!("agent `{agent}`: {}", diag.message())),
        );
        agent_body = Some(frontmatter.body().to_string());
        profile = parsed_profile;
    } else {
        profile = empty_agent_profile();
        agent_body = None;
    }

    let effective_project_config = load_effective_project_config_or_default(&ctx.project_root)?;
    if let Some(message) = crate::compiler::agent_copy::deprecated_fanout_agents_warning(
        effective_project_config.settings.meridian_agent_copy(),
    ) {
        warnings.push(message);
    }
    let lock = crate::lock::load_for_runtime_aliases(&ctx.project_root)?;
    let runtime_aliases = crate::models::merged_runtime_aliases(
        &lock.dependency_model_aliases,
        Some(&effective_project_config.models),
    );

    let policy = resolve_policy(
        &effective_project_config,
        PolicyInput {
            project_root: &ctx.project_root,
            runtime_aliases: &runtime_aliases,
            agent: request.agent.as_deref(),
            profile: &profile,
            model_override: request.model.as_deref(),
            harness_override: request.harness.as_deref(),
            effort_override: request.effort.as_deref(),
            approval_override: request.approval.as_deref(),
            sandbox_override: request.sandbox.as_deref(),
            models_refresh: request.models_refresh,
        },
    )?;

    warnings.extend(policy.warnings);

    let mars_dir = ctx.project_root.join(".mars");
    let effective_skills = resolve_effective_skills(&profile, &policy.routing.harness)?;

    let prompt = compile_prompt_surface(
        &mars_dir,
        agent_body.as_deref().unwrap_or(""),
        &effective_skills,
        &request.extra_skills,
        &policy.routing.harness,
        &policy.routing.model_token,
        &policy.routing.model,
        &profile.subagents,
        effective_project_config.settings.meridian_fanout_agents(),
    )?;

    warnings.extend(prompt.warnings);
    let (resolved_tools, tool_warnings) = resolve_bundle_tools(&profile, &policy.routing.harness)?;
    warnings.extend(tool_warnings);

    Ok(LaunchBundle {
        version: LAUNCH_BUNDLE_VERSION,
        agent: request.agent,
        agent_body,
        routing: policy.routing,
        execution_policy: policy.execution_policy,
        prompt_surface: bundle::PromptSurface {
            system_instruction: prompt.system_instruction,
            supplemental_documents: prompt.supplemental_documents,
            inventory_prompt: prompt.inventory_prompt,
        },
        scaffold_slots: ScaffoldSlots::placeholders(),
        tools: resolved_tools,
        skills: Skills {
            loaded: prompt.loaded_skills,
            available: prompt.available_skills,
            missing: prompt.missing_skills,
        },
        provenance: policy.provenance,
        warnings,
    })
}

fn empty_agent_profile() -> AgentProfile {
    AgentProfile {
        name: None,
        description: None,
        harness: None,
        model: None,
        mode: None,
        model_invocable: true,
        approval: None,
        sandbox: None,
        effort: None,
        autocompact: None,
        autocompact_pct: None,
        skills: SkillsSpec::default(),
        subagents: Vec::new(),
        tools: Vec::new(),
        tools_denied: Vec::new(),
        disallowed_tools: Vec::new(),
        mcp_tools: Vec::new(),
        harness_overrides: Default::default(),
        model_policies: Vec::new(),
        fanout: Vec::new(),
    }
}

fn load_effective_project_config_or_default(
    project_root: &std::path::Path,
) -> Result<EffectiveProjectConfig, MarsError> {
    match crate::config::load_effective_project_config(project_root) {
        Ok(config) => Ok(config),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {
            Ok(EffectiveProjectConfig::default())
        }
        Err(err) => Err(err),
    }
}

fn agent_file_path(project_root: &std::path::Path, agent: &str) -> PathBuf {
    project_root
        .join(".mars")
        .join("agents")
        .join(format!("{agent}.md"))
}

fn resolve_bundle_tools(
    profile: &crate::compiler::agents::AgentProfile,
    harness: &str,
) -> Result<(ToolsSpec, Vec<String>), MarsError> {
    let harness_kind = parse_harness_kind(harness)?;

    let effective_tools = profile.effective_tool_policy(&harness_kind);
    let mut warnings = Vec::new();

    let allowed = normalize_and_dedupe_tools(
        &effective_tools.allowed,
        harness,
        ToolPolicyKind::Allowed,
        &mut warnings,
    );
    let disallowed = normalize_and_dedupe_tools(
        &effective_tools.disallowed,
        harness,
        ToolPolicyKind::Disallowed,
        &mut warnings,
    );

    Ok((
        ToolsSpec {
            allowed,
            disallowed,
            mcp: effective_tools.mcp,
        },
        warnings,
    ))
}

fn normalize_and_dedupe_tools(
    tools: &[String],
    harness: &str,
    kind: ToolPolicyKind,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut projected = Vec::new();

    for tool in tools {
        let normalized = normalize_tool_for_harness(tool, harness);
        if normalized.status == ToolProjectionStatus::Unknown && is_first_class_harness(harness) {
            match kind {
                ToolPolicyKind::Allowed => warnings.push(format!(
                    "tool '{tool}' is not a known {harness} tool; passing through verbatim"
                )),
                ToolPolicyKind::Disallowed => continue,
            }
        }

        let trimmed = normalized.name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            projected.push(trimmed.to_string());
        }
    }

    projected
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolPolicyKind {
    Allowed,
    Disallowed,
}

fn resolve_effective_skills(
    profile: &crate::compiler::agents::AgentProfile,
    harness: &str,
) -> Result<SkillsSpec, MarsError> {
    let harness_kind = parse_harness_kind(harness)?;
    Ok(profile.effective_skills(&harness_kind).clone())
}

fn parse_harness_kind(harness: &str) -> Result<HarnessKind, MarsError> {
    HarnessKind::from_str(harness).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "invalid harness `{harness}` for launch bundle resolution; expected one of: claude, codex, opencode, cursor, pi"
            ),
        })
    })
}
