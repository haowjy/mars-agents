//! `mars agents` — list and inspect agents from the .mars/ canonical store.

use crate::compiler::agents::{parse_agent_content, parse_agent_profile};
use crate::error::MarsError;
use crate::frontmatter;
use crate::lock::ItemKind;

use super::output;

#[derive(serde::Serialize)]
struct AgentEntry {
    name: String,
    description: String,
    mode: String,
}

/// Arguments for `mars agents`.
#[derive(Debug, clap::Args)]
pub struct AgentsArgs {
    /// Filter by mode (primary or subagent).
    ///
    /// Global so it is accepted both as `mars agents --mode ...` and on the
    /// `list` subcommand (`mars agents list --mode ...`).
    #[arg(long, global = true)]
    pub mode: Option<String>,

    /// Filter by source name.
    #[arg(long, global = true)]
    pub source: Option<String>,

    #[command(subcommand)]
    pub command: Option<AgentsCommand>,
}

#[derive(Debug, clap::Subcommand)]
pub enum AgentsCommand {
    /// List all agents (same as bare `mars agents`).
    List,
    /// Show full metadata for a named agent.
    Show {
        /// Agent name.
        name: String,
    },
}

/// Run `mars agents`.
pub fn run(args: &AgentsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        Some(AgentsCommand::List) => run_list(args, ctx, json),
        Some(AgentsCommand::Show { name }) => run_show(name, ctx, json),
        None => run_list(args, ctx, json),
    }
}

fn run_list(args: &AgentsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let mars_dir = ctx.project_root.join(".mars");

    let mut entries: Vec<AgentEntry> = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Agent {
            continue;
        }

        // source filter
        if let Some(ref filter_source) = args.source
            && item.source != *filter_source
        {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let content = match std::fs::read_to_string(&disk_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let fm = match frontmatter::parse(&content) {
            Ok(fm) => fm,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let profile = parse_agent_profile(&fm, &mut diags);

        // mode filter
        let mode_str = match &profile.mode {
            Some(m) => m.as_str().to_string(),
            None => String::new(),
        };
        if let Some(ref filter_mode) = args.mode
            && mode_str != *filter_mode
        {
            continue;
        }

        let name = profile
            .name
            .clone()
            .unwrap_or_else(|| path_stem(&disk_path));
        let description = profile.description.clone().unwrap_or_default();

        entries.push(AgentEntry {
            name,
            description,
            mode: mode_str,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        output::print_json(&serde_json::json!({ "agents": entries }));
    } else {
        if entries.is_empty() {
            println!("  no agents");
        } else {
            // Compute column widths
            let name_w = entries
                .iter()
                .map(|e| e.name.len())
                .max()
                .unwrap_or(4)
                .max(4);
            let mode_w = entries
                .iter()
                .map(|e| e.mode.len())
                .max()
                .unwrap_or(4)
                .max(4);
            println!("{:<name_w$}  {:<mode_w$}  DESCRIPTION", "NAME", "MODE");
            for e in &entries {
                println!(
                    "{:<name_w$}  {:<mode_w$}  {}",
                    e.name, e.mode, e.description
                );
            }
        }
    }

    Ok(0)
}

fn run_show(name: &str, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let mars_dir = ctx.project_root.join(".mars");

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Agent {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let content = match std::fs::read_to_string(&disk_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let (profile, _fm) = match parse_agent_content(&content, &mut diags) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };

        let stem = path_stem(&disk_path);
        let agent_name = profile.name.as_deref().unwrap_or(stem.as_str());
        if !agent_name.eq_ignore_ascii_case(name) {
            continue;
        }

        let mode_str = profile.mode.as_ref().map(|m| m.as_str()).unwrap_or("");
        let harness_str = profile
            .harness
            .as_ref()
            .map(|h| h.to_harness_id().as_str())
            .unwrap_or("");
        let model_str = profile.model.as_deref().unwrap_or("");
        let approval_str = profile
            .approval
            .as_ref()
            .map(|a| a.as_str())
            .unwrap_or_default();
        let sandbox_str = profile
            .sandbox
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or_default();
        let effort_str = profile
            .effort
            .as_ref()
            .map(|e| e.as_str())
            .unwrap_or_default();
        let description_str = profile.description.as_deref().unwrap_or("");

        if json {
            output::print_json(&serde_json::json!({
                "name": agent_name,
                "description": description_str,
                "mode": mode_str,
                "harness": harness_str,
                "model": model_str,
                "skills": profile.skills.all(),
                "skills_structured": profile.skills,
                "subagents": profile.subagents,
                "approval": approval_str,
                "sandbox": sandbox_str,
                "effort": effort_str,
                "tools": profile.tools,
                "disallowed-tools": profile.disallowed_tools,
                "tools-denied": profile.tools_denied,
            }));
        } else {
            println!("name:        {agent_name}");
            println!("description: {description_str}");
            println!("mode:        {mode_str}");
            println!("harness:     {harness_str}");
            println!("model:       {model_str}");
            println!("approval:    {approval_str}");
            println!("sandbox:     {sandbox_str}");
            println!("effort:      {effort_str}");
            print_str_list("skills.load", &profile.skills.load);
            print_str_list("skills.available", &profile.skills.available);
            print_str_list("subagents", &profile.subagents);
            print_str_list("tools", &profile.tools);
            print_str_list("disallowed-tools", &profile.disallowed_tools);
            print_str_list("tools-denied", &profile.tools_denied);
        }

        return Ok(0);
    }

    eprintln!("error: agent `{name}` not found");
    Ok(1)
}

fn print_str_list(label: &str, items: &[String]) {
    if items.is_empty() {
        println!("{label}:        (none)");
    } else {
        println!("{label}:        {}", items.join(", "));
    }
}

fn path_stem(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod filter_flag_tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    fn agents_args(args: &[&str]) -> super::AgentsArgs {
        match Cli::try_parse_from(args).expect("should parse").command {
            Command::Agents(a) => a,
            other => panic!("expected agents command, got {other:?}"),
        }
    }

    #[test]
    fn mode_filter_populates_on_both_bare_and_list_forms() {
        // The `list` subcommand form is the discoverable one and must work.
        for args in [
            ["mars", "agents", "list", "--mode", "subagent"].as_slice(),
            ["mars", "agents", "--mode", "subagent", "list"].as_slice(),
            ["mars", "agents", "--mode", "subagent"].as_slice(),
        ] {
            let parsed = agents_args(args);
            // Value must actually populate (run_list reads args.mode), not merely parse.
            assert_eq!(parsed.mode.as_deref(), Some("subagent"), "args: {args:?}");
        }
    }

    #[test]
    fn source_filter_populates_on_list_form() {
        let parsed = agents_args(&["mars", "agents", "list", "--source", "core"]);
        assert_eq!(parsed.source.as_deref(), Some("core"));
    }
}
