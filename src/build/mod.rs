pub mod bundle;
pub mod inventory;
pub mod policy;
pub mod prompt;

use std::path::PathBuf;

use crate::compiler::tool_names::NativeStatus;
use crate::compiler::tool_policy::{RuleOrigin, ToolAction, ToolRule, project_rule};
use crate::content::ContentRoots;
use bundle::{LaunchBundle, ScaffoldSlots, Skills, ToolRuleSpec, ToolsSpec};
use policy::{PolicyInput, resolve_policy};
use prompt::compile_prompt_surface;

use crate::cli::MarsContext;
use crate::compiler::agents::{AgentProfile, parse_agent_content};
use crate::compiler::harness_descriptor::CompilerHarnessDescriptor;
use crate::config::EffectiveProjectConfig;
use crate::error::{ConfigError, MarsError};
use crate::frontmatter::SkillsSpec;

pub const LAUNCH_BUNDLE_VERSION: u32 = 4;

pub struct LaunchBundleRequest {
    pub agent: Option<String>,
    pub fallback_root: Option<PathBuf>,
    pub model: Option<String>,
    pub literal_model: bool,
    pub harness: Option<String>,
    pub excluded_harnesses: Vec<crate::harness::registry::HarnessId>,
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub extra_skills: Vec<String>,
    pub models_refresh: crate::models::ModelsRefreshControl,
}

pub fn build_launch_bundle(
    ctx: &MarsContext,
    mut request: LaunchBundleRequest,
) -> Result<LaunchBundle, MarsError> {
    let mut seen = std::collections::HashSet::new();
    request
        .excluded_harnesses
        .retain(|harness| seen.insert(*harness));
    let mut warnings: Vec<String> = Vec::new();
    let profile: AgentProfile;
    let agent_body: Option<String>;
    let agent_source_path: Option<PathBuf>;
    let content_roots = ContentRoots::new(&ctx.project_root, request.fallback_root.as_deref());

    if let Some(agent) = request.agent.as_deref() {
        let agent_path = content_roots.agent_file_path(agent);
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
        agent_source_path = Some(agent_path);
        profile = parsed_profile;
    } else {
        profile = empty_agent_profile();
        agent_body = None;
        agent_source_path = None;
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
            literal_model: request.literal_model,
            harness_override: request.harness.as_deref(),
            excluded_harnesses: &request.excluded_harnesses,
            effort_override: request.effort.as_deref(),
            approval_override: request.approval.as_deref(),
            sandbox_override: request.sandbox.as_deref(),
            models_refresh: request.models_refresh,
        },
    )?;

    warnings.extend(policy.warnings);
    let with_selection_report = |err: MarsError| match err {
        MarsError::Config(_) => MarsError::Selection {
            code: "invalid_config",
            message: err.to_string(),
            report: Box::new(policy.routing.route_trace.clone()),
        },
        _ => err,
    };

    let effective_skills = resolve_effective_skills(&profile, &policy.routing.harness)
        .map_err(with_selection_report)?;

    let prompt = compile_prompt_surface(
        &content_roots,
        agent_body.as_deref().unwrap_or(""),
        &effective_skills,
        &request.extra_skills,
        &policy.routing.harness,
        &policy.routing.model_token,
        &policy.routing.model,
        &profile.subagents,
        effective_project_config.settings.meridian_fanout_agents(),
    )
    .map_err(with_selection_report)?;

    warnings.extend(prompt.warnings);
    let (resolved_tools, tool_warnings) =
        resolve_bundle_tools(&profile, &policy.routing.harness).map_err(with_selection_report)?;
    warnings.extend(tool_warnings);

    Ok(LaunchBundle {
        version: LAUNCH_BUNDLE_VERSION,
        agent: request.agent,
        agent_source_path,
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
        user_invocable: true,
        had_model_invocable_field: false,
        had_user_invocable_field: false,
        approval: None,
        sandbox: None,
        effort: None,
        autocompact: None,
        autocompact_pct: None,
        skills: SkillsSpec::default(),
        subagents: Vec::new(),
        tools: Default::default(),
        disallowed_tools: Vec::new(),
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

fn resolve_bundle_tools(
    profile: &crate::compiler::agents::AgentProfile,
    harness: &str,
) -> Result<(ToolsSpec, Vec<String>), MarsError> {
    let descriptor = parse_harness_descriptor(harness)?;
    let harness_kind = descriptor.kind;
    let harness = descriptor.canonical_id;

    let policy = profile.effective_tool_policy(&harness_kind);
    let mut warnings = Vec::new();
    let mut rules = Vec::new();
    let mut allowed = Vec::new();
    let mut mcp = Vec::new();
    // Built-in tools before MCP refs, as the flat lists have always been ordered.
    let mut disallowed_builtin = Vec::new();
    let mut disallowed_mcp = Vec::new();

    for rule in &policy.rules {
        let projected = project_rule(rule, harness_kind);
        if projected.status == NativeStatus::Unknown {
            warnings.push(unknown_tool_warning(rule, projected.note, harness));
        }

        let flat = match (rule.action, rule.is_mcp()) {
            (ToolAction::Allow, false) => Some(&mut allowed),
            (ToolAction::Allow, true) => Some(&mut mcp),
            (ToolAction::Deny, false) => Some(&mut disallowed_builtin),
            (ToolAction::Deny, true) => Some(&mut disallowed_mcp),
            // The flat lists have no `ask`; only `rules` carries it.
            (ToolAction::Ask, _) => None,
        };
        if let Some(flat) = flat {
            for native in &projected.native {
                if !flat.contains(native) {
                    flat.push(native.clone());
                }
            }
        }

        rules.push(ToolRuleSpec {
            capability: rule.capability.clone(),
            pattern: rule.pattern.clone(),
            action: rule.action.as_str(),
            native: projected.native,
            native_status: projected.status.as_str(),
            origin: rule.origin.map(RuleOrigin::as_str),
        });
    }

    let mut disallowed = disallowed_builtin;
    for native in disallowed_mcp {
        if !disallowed.contains(&native) {
            disallowed.push(native);
        }
    }

    Ok((
        ToolsSpec {
            rules,
            allowed,
            disallowed,
            mcp,
        },
        warnings,
    ))
}

/// Why Mars could not name `rule` for `harness`. The rule still ships in `tools.rules` with
/// `native_status: unknown`.
fn unknown_tool_warning(rule: &ToolRule, note: Option<&'static str>, harness: &str) -> String {
    let denied = rule.action == ToolAction::Deny;
    if rule.is_mcp() {
        format!(
            "{}MCP ref `mcp({})` cannot be represented for {harness}: {}",
            if denied { "disallowed " } else { "" },
            rule.pattern.as_deref().unwrap_or_default(),
            note.unwrap_or("unsupported")
        )
    } else {
        format!(
            "{}tool '{}' has no known {harness} name; Mars left it out (native_status: unknown)",
            if denied { "disallowed " } else { "" },
            rule.render()
        )
    }
}

fn resolve_effective_skills(
    profile: &crate::compiler::agents::AgentProfile,
    harness: &str,
) -> Result<SkillsSpec, MarsError> {
    let descriptor = parse_harness_descriptor(harness)?;
    Ok(profile.effective_skills(&descriptor.kind).clone())
}

fn parse_harness_descriptor(
    harness: &str,
) -> Result<&'static CompilerHarnessDescriptor, MarsError> {
    crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness).ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!(
                "invalid harness `{harness}` for launch bundle resolution; expected one of: {}",
                crate::compiler::harness_descriptor::known_canonical_ids()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
    })
}
