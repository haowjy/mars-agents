pub(crate) mod agent_rules;
pub mod bundle;
pub mod plugin;
pub mod policy;
pub mod prompt;

use std::path::PathBuf;

use crate::compiler::tool_policy::{
    NativeStatus, RuleKind, ToolPolicyRuntime, project_platform_capability,
};
use crate::content::ContentRoots;
use bundle::{CapabilityNativeSpec, LaunchBundle, ScaffoldSlots, Skills, ToolRuleSpec, ToolsSpec};
use mars_schema::tool_policy::ToolAction;
use policy::{PolicyInput, resolve_policy};
use prompt::compile_prompt_surface;

use crate::cli::MarsContext;
use crate::compiler::harness_descriptor::CompilerHarnessDescriptor;
use crate::config::EffectiveProjectConfig;
use crate::diagnostic::Diagnostic;
use crate::error::{ConfigError, MarsError};
use mars_schema::agent::AgentProfile;
use mars_schema::frontmatter::SkillsSpec;
use mars_schema::network::{NetworkHost, NetworkPolicy};

pub const LAUNCH_BUNDLE_VERSION: u32 = 4;

pub struct LaunchBundleRequest {
    pub agent: Option<String>,
    pub fallback_root: Option<PathBuf>,
    pub model: Option<String>,
    pub literal_model: bool,
    pub harness: Option<String>,
    pub excluded_harnesses: Vec<mars_schema::HarnessId>,
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub sandbox: Option<String>,
    pub network: Option<NetworkPolicy>,
    pub network_allow: Vec<NetworkHost>,
    pub network_deny: Vec<NetworkHost>,
    pub extra_skills: Vec<String>,
    /// Replaces the agent's profile and overlay tool rules; see `--agent-tool-rules`.
    pub agent_tool_rules: Option<Vec<mars_schema::tool_policy::ToolRule>>,
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
    // Everything found while building. The bundle carries the messages of those that are shown
    // without being asked; failures return an error instead.
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut profile: AgentProfile;
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

        let valid = mars_schema::read::installed_agent(&agent_content)
            .into_valid()
            .map_err(|error| {
                MarsError::Config(ConfigError::Invalid {
                    message: format!(
                        "agent `{agent}` has invalid frontmatter in {}: {}",
                        agent_path.display(),
                        error.message()
                    ),
                })
            })?;

        diagnostics.extend(valid.warnings.iter().map(|diag| {
            Diagnostic::from_schema(diag, format!("agent `{agent}`: {}", diag.message()))
        }));
        agent_body = Some(valid.body);
        agent_source_path = Some(agent_path);
        profile = valid.fields;
    } else {
        profile = empty_agent_profile();
        agent_body = None;
        agent_source_path = None;
    }

    let effective_project_config = load_effective_project_config_or_default(&ctx.project_root)?;
    if let Some(diagnostic) = crate::diagnostic::compatibility_preflight(
        effective_project_config
            .settings
            .min_mars_version
            .as_deref(),
    )
    .map_err(|message| ConfigError::Invalid { message })?
    {
        return Err(ConfigError::Invalid {
            message: format!("[{}] {}", diagnostic.code, diagnostic.message),
        }
        .into());
    }
    diagnostics.extend(effective_project_config.diagnostics.iter().cloned());
    let overlay = request
        .agent
        .as_deref()
        .and_then(|agent| effective_project_config.agents.get(agent))
        .map(|overlay| &overlay.tools);
    let effective_tools = match request.agent_tool_rules.take() {
        Some(rules) => crate::compiler::effective_tools::effective_tools_for_rules(
            &effective_project_config.settings.tools,
            rules,
            overlay,
        ),
        None => crate::compiler::effective_tools::effective_tools(
            &effective_project_config.settings.tools,
            &profile.tools,
            overlay,
        ),
    };
    profile.tools = effective_tools;
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
            network_override: request.network.as_ref(),
            network_allow: &request.network_allow,
            network_deny: &request.network_deny,
            models_refresh: request.models_refresh,
        },
    )?;

    diagnostics.extend(
        policy
            .warnings
            .into_iter()
            .map(|warning| Diagnostic::warning("launch.warning", warning)),
    );

    let prompt = compile_prompt_surface(
        &content_roots,
        agent_body.as_deref().unwrap_or(""),
        &profile.skills,
        &request.extra_skills,
        &policy.routing.harness,
        &policy.routing.model_token,
        &policy.routing.model,
    );

    diagnostics.extend(prompt.warnings);
    let resolved_tools = resolve_bundle_tools(&profile, &policy.routing.harness)?;

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
        },
        scaffold_slots: ScaffoldSlots::placeholders(),
        tools: resolved_tools,
        skills: Skills {
            loaded: prompt.loaded_skills,
            available: prompt.available_skills,
            missing: prompt.missing_skills,
        },
        provenance: policy.provenance,
        warnings: diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.shown(false))
            .map(|diagnostic| diagnostic.message)
            .collect(),
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
        network: None,
        network_allow: Default::default(),
        network_deny: Default::default(),
        effort: None,
        skills: SkillsSpec::default(),
        subagents: Vec::new(),
        tools: Default::default(),
        harness_overrides: Default::default(),
        model_policies: Vec::new(),
    }
}

pub(crate) fn load_effective_project_config_or_default(
    project_root: &std::path::Path,
) -> Result<EffectiveProjectConfig, MarsError> {
    match crate::config::load_effective_project_config(project_root) {
        Ok(config) => Ok(config),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {
            crate::config::load_user_effective_project_config()
        }
        Err(err) => Err(err),
    }
}

/// The agent's tool rules for `harness`. A permission gap is never a warning: Meridian prints
/// `warnings[]` at every launch, so a rule Mars cannot name or enforce goes out with
/// `native_status: unknown` and a `note`, and the consumer reports it. `mars sync` is where an
/// author hears about a typo.
fn resolve_bundle_tools(
    profile: &mars_schema::agent::AgentProfile,
    harness: &str,
) -> Result<ToolsSpec, MarsError> {
    let descriptor = parse_harness_descriptor(harness)?;
    let projection = profile.tools.project(descriptor.kind);

    let rules = projection
        .rules()
        .iter()
        .map(|(rule, projected)| {
            let native = projected.native();
            ToolRuleSpec {
                rule: (*rule).clone(),
                native,
                native_tools: projected.native_tools(),
                native_status: projected.status.map(NativeStatus::as_str),
                note: projected
                    .covered_by
                    .as_ref()
                    .map(|by| format!("never applies: `{by}` covers it"))
                    .or_else(|| projected.note().map(str::to_string)),
                covered_by: projected.covered_by.clone(),
            }
        })
        .collect();

    let capability_natives = ["agent", "task"]
        .into_iter()
        .map(|capability| {
            let projected = project_platform_capability(capability, descriptor.kind);
            let status = projected
                .status
                .expect("a builtin capability always has a native status");
            (
                capability.to_string(),
                CapabilityNativeSpec {
                    native: projected.native(),
                    native_status: status.as_str(),
                    note: status.note(),
                },
            )
        })
        .collect();

    Ok(ToolsSpec {
        rules,
        set_aside: Vec::new(),
        capability_natives,
        // The flat lists have no `ask`, `*` or allowlist; only `rules` carries those.
        allowed: projection.natives(ToolAction::Allow, RuleKind::Builtin),
        disallowed: projection.natives(ToolAction::Deny, RuleKind::All),
        mcp: projection.natives(ToolAction::Allow, RuleKind::Mcp),
    })
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
