//! `mars build` — build artifacts from static project state.

use clap::{ArgAction, ValueEnum};

use crate::build::{LaunchBundleRequest, build_launch_bundle};
use crate::cli::MarsContext;
use crate::error::MarsError;

#[derive(Debug, clap::Args)]
pub struct BuildArgs {
    #[command(subcommand)]
    pub command: BuildCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum BuildCommand {
    /// Build a harness-targeted launch scaffold/bundle for an agent or ad-hoc launch.
    LaunchBundle(LaunchBundleArgs),
}

#[derive(Debug, Clone, ValueEnum)]
enum HarnessArg {
    Claude,
    Codex,
    Opencode,
    Cursor,
    Pi,
}

impl HarnessArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Cursor => "cursor",
            Self::Pi => "pi",
        }
    }
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
    Yolo,
}

impl ApprovalArg {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::Confirm => "confirm",
            Self::Yolo => "yolo",
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
    /// Omit for ad-hoc mode; then `--model` is required.
    #[arg(long)]
    pub agent: Option<String>,

    /// Override model token or canonical model id.
    #[arg(long)]
    pub model: Option<String>,

    /// Override harness target.
    #[arg(long, value_enum)]
    harness: Option<HarnessArg>,

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
}

pub fn run(args: &BuildArgs, ctx: &MarsContext, _json: bool) -> Result<i32, MarsError> {
    match &args.command {
        BuildCommand::LaunchBundle(subargs) => run_launch_bundle(subargs, ctx),
    }
}

fn run_launch_bundle(args: &LaunchBundleArgs, ctx: &MarsContext) -> Result<i32, MarsError> {
    if args.agent.is_none() && args.model.is_none() {
        return Err(MarsError::InvalidRequest {
            message: "ad-hoc launch-bundle requires --model".to_string(),
        });
    }

    let bundle = build_launch_bundle(
        ctx,
        LaunchBundleRequest {
            agent: args.agent.clone(),
            model: args.model.clone(),
            harness: args.harness.as_ref().map(|h| h.as_str().to_string()),
            effort: args.effort.as_ref().map(|e| e.as_str().to_string()),
            approval: args.approval.as_ref().map(|a| a.as_str().to_string()),
            sandbox: args.sandbox.as_ref().map(|s| s.as_str().to_string()),
            extra_skills: args.extra_skills.clone(),
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
