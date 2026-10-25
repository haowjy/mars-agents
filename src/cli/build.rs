//! `mars build` — build artifacts from static project state.

use clap::{ArgAction, ValueEnum};
use std::path::PathBuf;

use crate::build::{LaunchBundleRequest, build_launch_bundle};
use crate::cli::MarsContext;
use crate::error::MarsError;
use mars_schema::HarnessId;

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
}

fn parse_harness(value: &str) -> Result<HarnessId, String> {
    mars_schema::harness::parse(value).ok_or_else(|| {
        format!(
            "unknown harness `{value}`; expected one of: {}",
            mars_schema::harness::NAMES.join(", ")
        )
    })
}

#[derive(Debug, Clone, ValueEnum)]
enum EffortArg {
    Low,
    Medium,
    High,
    Xhigh,
}

impl EffortArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
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

    /// Add extra skills by name. Supports `--skill a --skill b` and `--skill a,b`.
    #[arg(long = "skill", value_delimiter = ',', action = ArgAction::Append)]
    pub extra_skills: Vec<String>,

    /// Refresh models.dev catalog and harness probes synchronously before building (blocks until complete).
    #[arg(long, conflicts_with = "no_refresh_models")]
    pub refresh_models: bool,

    /// Use disk-only catalog/probe caches; launch no background refresh worker.
    #[arg(long, conflicts_with = "refresh_models")]
    pub no_refresh_models: bool,
}

pub fn run(args: &BuildArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        BuildCommand::LaunchBundle(subargs) => run_launch_bundle(subargs, ctx),
        BuildCommand::Plugin(subargs) => run_plugin(subargs, ctx, json),
    }
}

fn run_plugin(args: &PluginArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let package_root = match &args.path {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => std::env::current_dir()?.join(path),
        None => ctx.project_root.clone(),
    };
    let manifest = crate::config::load(&package_root)?;
    let harnesses = manifest
        .plugin
        .as_ref()
        .map(|plugin| plugin.harnesses.clone())
        .unwrap_or_else(crate::config::default_plugin_harnesses);
    let destination = args
        .out
        .clone()
        .map(crate::build::plugin::PluginBuildDestination::Out)
        .unwrap_or(crate::build::plugin::PluginBuildDestination::Configured);
    let report = crate::build::plugin::build_plugin(&crate::build::plugin::PluginBuildRequest {
        package_root,
        destination,
        harnesses,
        version: None,
    })?;

    if json {
        crate::cli::output::print_json(&report);
    } else {
        crate::cli::output::print_diagnostics(&report.diagnostics);
        for tree in &report.trees {
            crate::cli::output::print_success(&format!(
                "built plugin `{}` tree `{}` at {} ({} agents, {} skills, {} hooks, {} MCP servers)",
                report.plugin_name,
                tree.tree.as_str(),
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
            extra_skills: args.extra_skills.clone(),
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
