//! `mars build` — build artifacts from static project state.

use clap::{ArgAction, ValueEnum};
use std::path::PathBuf;

use crate::build::{LaunchBundleRequest, build_launch_bundle};
use crate::cli::MarsContext;
use crate::error::MarsError;
use mars_schema::HarnessId;
use mars_schema::network::{NetworkHost, NetworkPolicy};

#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    #[command(subcommand)]
    pub command: BuildCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum BuildCommand {
    /// Build a harness-targeted launch scaffold/bundle for an agent or ad-hoc launch.
    LaunchBundle(LaunchBundleArgs),

    /// Build plugin trees from a package and its resolved dependency graph.
    Plugin(PluginArgs),
}

#[derive(Debug, clap::Args)]
pub struct PluginArgs {
    /// Package directory containing mars.toml (default: detected project root).
    pub path: Option<PathBuf>,

    /// Destination for the generated plugin output.
    #[arg(long)]
    pub out: Option<PathBuf>,

    /// Show every field a plugin tree does not carry, not only what you can act on.
    #[arg(long)]
    pub verbose: bool,
}

fn parse_harness(value: &str) -> Result<HarnessId, String> {
    mars_schema::harness::parse(value).ok_or_else(|| {
        format!(
            "unknown harness `{value}`; expected one of: {}",
            mars_schema::harness::names()
        )
    })
}

#[derive(Debug, Clone, ValueEnum)]
enum EffortArg {
    None,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl EffortArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum ApprovalArg {
    Default,
    Auto,
    Confirm,
    Never,
}

impl ApprovalArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::Confirm => "confirm",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum SandboxArg {
    Default,
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct LaunchBundleArgs {
    /// Agent name from `.mars/agents/<name>.md`.
    /// Omit for ad-hoc mode; without `--model`, the resolved harness uses its default model.
    #[arg(long)]
    pub agent: Option<String>,

    /// Secondary Mars project used only for missing agents and skills.
    #[arg(long)]
    pub fallback_root: Option<PathBuf>,

    /// Override model token or canonical model id.
    #[arg(long)]
    pub model: Option<String>,

    /// Treat --model as a literal ID, bypassing aliases; an empty ID pins harness default.
    #[arg(long, requires = "model")]
    literal_model: bool,

    /// Override harness target.
    #[arg(long, value_parser = parse_harness)]
    harness: Option<HarnessId>,

    /// Exclude a harness from this invocation (repeatable); cannot expand configured targets.
    #[arg(long = "exclude-harness", value_parser = parse_harness, action = ArgAction::Append)]
    excluded_harnesses: Vec<HarnessId>,

    /// Override effort level.
    #[arg(long, value_enum)]
    effort: Option<EffortArg>,

    /// Override approval mode.
    #[arg(long, value_enum)]
    approval: Option<ApprovalArg>,

    /// Override sandbox mode.
    #[arg(long, value_enum)]
    sandbox: Option<SandboxArg>,

    /// Override shell network access: allow, deny, or allowlist.
    #[arg(long)]
    network: Option<NetworkPolicy>,

    /// Add a host to the network allow set (repeatable).
    #[arg(long = "network-allow", action = ArgAction::Append)]
    network_allow: Vec<NetworkHost>,

    /// Add a host to the network deny set (repeatable).
    #[arg(long = "network-deny", action = ArgAction::Append)]
    network_deny: Vec<NetworkHost>,

    /// Add extra skills by name. Supports `--skill a --skill b` and `--skill a,b`.
    #[arg(long = "skill", value_delimiter = ',', action = ArgAction::Append)]
    pub extra_skills: Vec<String>,

    /// JSON file of tool rules that replace the agent's own profile and overlay rules; `-` reads
    /// stdin. An array of `tools.rules` entries (`capability`, `pattern`, `action`, `origin`);
    /// `project` entries are skipped. Mars then applies the current `[settings.tools]`.
    #[arg(long, value_name = "PATH")]
    agent_tool_rules: Option<PathBuf>,

    /// Refresh models.dev catalog and harness probes synchronously before building (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    pub refresh_models: bool,

    /// Use disk-only catalog/probe caches; launch no background refresh worker.
    #[arg(long, conflicts_with = "refresh_models")]
    pub no_refresh_models: bool,
}

pub fn run(
    args: &BuildArgs,
    ctx: &MarsContext,
    explicit_root: Option<&std::path::Path>,
    json: bool,
) -> Result<i32, MarsError> {
    match &args.command {
        BuildCommand::LaunchBundle(subargs) => run_launch_bundle(subargs, ctx),
        BuildCommand::Plugin(subargs) => run_plugin(subargs, ctx, explicit_root, json),
    }
}

fn run_plugin(
    args: &PluginArgs,
    ctx: &MarsContext,
    explicit_root: Option<&std::path::Path>,
    json: bool,
) -> Result<i32, MarsError> {
    let package_root = match &args.path {
        Some(path) => super::resolve_cli_path(path, explicit_root)?,
        None => ctx.project_root.clone(),
    };
    let manifest = crate::config::load(&package_root)?;
    let metadata = crate::build::plugin::validate_plugin_build(&manifest)?;
    let destination = args
        .out
        .clone()
        .map(crate::build::plugin::PluginBuildDestination::Out)
        .unwrap_or(crate::build::plugin::PluginBuildDestination::Configured);
    let report = crate::build::plugin::build_plugin(&crate::build::plugin::PluginBuildRequest {
        package_root,
        destination,
        metadata,
        version: None,
        lossiness_mode: if args.verbose {
            crate::diagnostic::LossinessMode::Verbose
        } else {
            crate::diagnostic::LossinessMode::Surface
        },
    })?;

    if json {
        crate::cli::output::print_json(&report);
    } else {
        crate::cli::output::print_diagnostics(&report.diagnostics, args.verbose);
        for tree in &report.trees {
            crate::cli::output::print_success(&format!(
                "built plugin `{}` tree `{}` at {} ({} agents, {} skills, {} hooks, {} MCP servers)",
                report.plugin_name,
                tree.tree,
                tree.output.display(),
                tree.components.agents,
                tree.components.skills,
                tree.components.hooks,
                tree.components.mcp_servers,
            ));
        }
    }
    Ok(0)
}

fn run_launch_bundle(args: &LaunchBundleArgs, ctx: &MarsContext) -> Result<i32, MarsError> {
    let agent_tool_rules = args
        .agent_tool_rules
        .as_deref()
        .map(read_agent_tool_rules)
        .transpose()?;
    let models_refresh =
        crate::models::resolve_models_refresh_control(args.refresh_models, args.no_refresh_models)?;
    let bundle = build_launch_bundle(
        ctx,
        LaunchBundleRequest {
            agent: args.agent.clone(),
            fallback_root: args.fallback_root.clone(),
            model: args.model.clone(),
            literal_model: args.literal_model,
            harness: args.harness.as_ref().map(|h| h.as_str().to_string()),
            excluded_harnesses: args.excluded_harnesses.clone(),
            effort: args.effort.as_ref().map(|e| e.as_str().to_string()),
            approval: args.approval.as_ref().map(|a| a.as_str().to_string()),
            sandbox: args.sandbox.as_ref().map(|s| s.as_str().to_string()),
            network: args.network.clone(),
            network_allow: args.network_allow.clone(),
            network_deny: args.network_deny.clone(),
            extra_skills: args.extra_skills.clone(),
            agent_tool_rules,
            models_refresh,
        },
    )?;

    println!(
        "{}",
        serde_json::to_string_pretty(&bundle).map_err(|err| MarsError::Internal(format!(
            "failed to serialize launch bundle: {err}"
        )))?
    );

    Ok(0)
}

fn read_agent_tool_rules(
    path: &std::path::Path,
) -> Result<Vec<mars_schema::tool_policy::ToolRule>, MarsError> {
    let json = if path == std::path::Path::new("-") {
        std::io::read_to_string(std::io::stdin()).map_err(|source| MarsError::Io {
            operation: "read --agent-tool-rules from stdin".to_string(),
            path: path.to_path_buf(),
            source,
        })?
    } else {
        std::fs::read_to_string(path).map_err(|source| MarsError::Io {
            operation: "read --agent-tool-rules".to_string(),
            path: path.to_path_buf(),
            source,
        })?
    };
    crate::build::agent_rules::parse_agent_tool_rules(&json)
}
