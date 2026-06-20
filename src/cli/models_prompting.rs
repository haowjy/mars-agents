//! CLI handler for `mars models prompting`.

use clap::Parser;
use indexmap::IndexMap;

use super::models_common::{
    load_merged_aliases, load_project_config_layers_optional, models_cache_ttl_hours,
};
use crate::build::policy::{PolicyInput, resolve_policy};
use crate::compiler::agents::{AgentProfile, parse_agent_content};
use crate::error::MarsError;
use crate::lock::ItemKind;
use crate::models::{self, ModelAlias};
use crate::types::MarsContext;

#[derive(Debug, Parser)]
#[command(
    after_help = "Examples:\n  mars models prompting @explorer\n  mars models prompting gpt55"
)]
pub struct PromptingArgs {
    /// Agent name/ref or model alias to look up prompting guidance for.
    pub reference: String,
    /// Refresh models.dev catalog and harness probes synchronously before resolving an agent.
    #[arg(long, conflicts_with = "no_refresh_models")]
    refresh_models: bool,
    /// Skip automatic models-cache refresh; use whatever is on disk.
    #[arg(long, conflicts_with = "refresh_models")]
    no_refresh_models: bool,
}

pub fn run(args: &PromptingArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let project_config = load_project_config_layers_optional(&ctx.project_root)?;
    let merged = load_merged_aliases(&ctx.project_root, project_config.as_ref())?;
    let refresh =
        models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;

    let target = resolve_prompt_ref(
        &args.reference,
        ctx,
        &merged,
        project_config.as_ref(),
        refresh,
    )?;

    if json {
        let out = target.to_json(&args.reference);
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else if target.found {
        print_prompt_target(&target);
    } else {
        eprintln!(
            "Unknown agent or model ref `{}`. Run `mars agents` or `mars models list` to see available refs.",
            args.reference
        );
        eprintln!("Examples:");
        eprintln!("  mars models prompting @explorer");
        eprintln!("  mars models prompting gpt55");
        return Ok(1);
    }

    Ok(if target.found { 0 } else { 1 })
}

#[derive(Debug)]
struct PromptTarget {
    found: bool,
    ref_kind: Option<PromptRefKind>,
    agent_name: Option<String>,
    model_alias: Option<String>,
    model_name: Option<String>,
    prompting: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum PromptRefKind {
    Agent,
    Model,
}

impl PromptRefKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Model => "model",
        }
    }
}

impl PromptTarget {
    fn unknown() -> Self {
        Self {
            found: false,
            ref_kind: None,
            agent_name: None,
            model_alias: None,
            model_name: None,
            prompting: None,
        }
    }

    fn to_json(&self, input_ref: &str) -> serde_json::Value {
        serde_json::json!({
            "ref": input_ref,
            "ref_kind": self.ref_kind.map(PromptRefKind::as_str),
            "agent_name": self.agent_name,
            "model_alias": self.model_alias,
            "model_name": self.model_name,
            "found": self.found,
            "prompting": self.prompting,
        })
    }
}

fn resolve_prompt_ref(
    input_ref: &str,
    ctx: &MarsContext,
    aliases: &IndexMap<String, ModelAlias>,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> Result<PromptTarget, MarsError> {
    if let Some(agent) = resolve_prompt_agent(input_ref, ctx)? {
        return prompt_target_for_agent(agent, ctx, aliases, project_config, refresh);
    }

    if input_ref.starts_with('@') {
        return Ok(PromptTarget::unknown());
    }

    Ok(aliases
        .get(input_ref)
        .map(|alias| prompt_target_for_model(input_ref, alias, ctx, project_config, refresh))
        .unwrap_or_else(PromptTarget::unknown))
}

struct PromptAgent {
    name: String,
    file_stem: String,
    profile: AgentProfile,
}

#[derive(Debug, Clone, Copy)]
enum PromptAgentMatch {
    FileStem,
    ProfileName,
}

fn resolve_prompt_agent(
    input_ref: &str,
    ctx: &MarsContext,
) -> Result<Option<PromptAgent>, MarsError> {
    let lookup_name = agent_ref_lookup_name(input_ref);
    let mut agents = load_prompt_agents(ctx)?;

    for match_kind in [PromptAgentMatch::FileStem, PromptAgentMatch::ProfileName] {
        if let Some(index) = agents
            .iter()
            .position(|agent| prompt_agent_matches(agent, lookup_name, match_kind))
        {
            return Ok(Some(agents.remove(index)));
        }
    }

    Ok(None)
}

fn load_prompt_agents(ctx: &MarsContext) -> Result<Vec<PromptAgent>, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let mars_dir = ctx.project_root.join(".mars");
    let mut agents = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Agent {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let content = match std::fs::read_to_string(&disk_path) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let (profile, _fm) = match parse_agent_content(&content, &mut diags) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };
        if let Some(fatal) = diags.iter().find(|diag| diag.is_error()) {
            eprintln!(
                "warning: skipping {}: {}",
                disk_path.display(),
                fatal.message()
            );
            continue;
        }

        let stem = prompt_path_stem(&disk_path);
        let agent_name = profile.name.as_deref().unwrap_or(stem.as_str());
        agents.push(PromptAgent {
            name: agent_name.to_string(),
            file_stem: stem,
            profile,
        });
    }

    Ok(agents)
}

fn prompt_agent_matches(
    agent: &PromptAgent,
    lookup_name: &str,
    match_kind: PromptAgentMatch,
) -> bool {
    match match_kind {
        PromptAgentMatch::FileStem => agent.file_stem.eq_ignore_ascii_case(lookup_name),
        PromptAgentMatch::ProfileName => agent.name.eq_ignore_ascii_case(lookup_name),
    }
}

fn agent_ref_lookup_name(input_ref: &str) -> &str {
    input_ref.strip_prefix('@').unwrap_or(input_ref)
}

fn prompt_path_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn prompt_target_for_agent(
    agent: PromptAgent,
    ctx: &MarsContext,
    aliases: &IndexMap<String, ModelAlias>,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> Result<PromptTarget, MarsError> {
    let effective_config = project_config
        .map(|loaded| loaded.effective.clone())
        .unwrap_or_default();
    let policy = resolve_policy(
        &effective_config,
        PolicyInput {
            project_root: &ctx.project_root,
            runtime_aliases: aliases,
            agent: Some(&agent.name),
            profile: &agent.profile,
            model_override: None,
            harness_override: None,
            effort_override: None,
            approval_override: None,
            sandbox_override: None,
            models_refresh: refresh,
        },
    )?;

    Ok(prompt_target_for_routing(
        Some(agent.name),
        PromptRefKind::Agent,
        &policy.routing,
        aliases,
    ))
}

fn prompt_target_for_routing(
    agent_name: Option<String>,
    ref_kind: PromptRefKind,
    routing: &crate::build::bundle::Routing,
    aliases: &IndexMap<String, ModelAlias>,
) -> PromptTarget {
    let token = routing.model_token.trim();
    let model_alias = (!token.is_empty() && aliases.contains_key(token)).then(|| token.to_string());
    let prompting = model_alias
        .as_deref()
        .and_then(|alias| aliases.get(alias))
        .and_then(|alias| alias.prompting.clone());
    let model_name = runnable_model_name(routing);

    PromptTarget {
        found: true,
        ref_kind: Some(ref_kind),
        agent_name,
        model_alias,
        model_name,
        prompting,
    }
}

fn runnable_model_name(routing: &crate::build::bundle::Routing) -> Option<String> {
    let harness_model = routing.harness_model.trim();
    if !harness_model.is_empty() {
        return Some(harness_model.to_string());
    }

    let model = routing.model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

fn prompt_target_for_model(
    alias_name: &str,
    alias: &ModelAlias,
    ctx: &MarsContext,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> PromptTarget {
    let cache = prompt_model_cache(ctx, project_config, refresh);
    PromptTarget {
        found: true,
        ref_kind: Some(PromptRefKind::Model),
        agent_name: None,
        model_alias: Some(alias_name.to_string()),
        model_name: Some(model_name_for_alias(alias_name, alias, &cache)),
        prompting: alias.prompting.clone(),
    }
}

fn prompt_model_cache(
    ctx: &MarsContext,
    project_config: Option<&crate::config::LoadedProjectConfig>,
    refresh: models::ModelsRefreshControl,
) -> models::ModelsCache {
    let mars_dir = ctx.project_root.join(".mars");
    let ttl = models_cache_ttl_hours(project_config);
    models::ensure_fresh(&mars_dir, ttl, refresh.catalog_mode)
        .map(|(cache, _)| cache)
        .or_else(|_| models::read_cache(&mars_dir))
        .unwrap_or(models::ModelsCache {
            models: Vec::new(),
            fetched_at: None,
        })
}

fn model_name_for_alias(
    alias_name: &str,
    alias: &ModelAlias,
    cache: &models::ModelsCache,
) -> String {
    models::resolve_model_id_for_alias(alias, cache)
        .unwrap_or_else(|| alias.pinned_model_id().unwrap_or(alias_name).to_string())
}

fn print_prompt_target(target: &PromptTarget) {
    if let Some(text) = target.prompting.as_deref() {
        println!("{text}");
        return;
    }

    match target.ref_kind {
        Some(PromptRefKind::Agent) => {
            let agent_name = target.agent_name.as_deref().unwrap_or("unknown");
            match target.model_alias.as_deref() {
                Some(model_alias) => {
                    println!(
                        "No prompting guidance defined for agent `{agent_name}` (model alias `{model_alias}`)."
                    );
                    print_prompting_field_hint(model_alias);
                }
                None => {
                    let model = target.model_name.as_deref().unwrap_or("no model");
                    println!(
                        "No prompting guidance defined for agent `{agent_name}` (model `{model}`)."
                    );
                    println!("Prompting guidance is read from a known model alias.");
                }
            }
        }
        Some(PromptRefKind::Model) => {
            let model_alias = target.model_alias.as_deref().unwrap_or("unknown");
            println!("No prompting guidance defined for model alias `{model_alias}`.");
            print_prompting_field_hint(model_alias);
        }
        None => {}
    }

    println!();
    println!("Examples:");
    println!("  mars models prompting @explorer");
    println!("  mars models prompting gpt55");
}

fn print_prompting_field_hint(model_alias: &str) {
    println!("Add a `prompting` field to the alias in mars.toml:");
    println!();
    println!("  [models.{model_alias}]");
    println!("  prompting = \"Prompting tips for this model.\"");
}
