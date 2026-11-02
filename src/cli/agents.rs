//! `mars agents` — list and inspect agents from the .mars/ canonical store.

use std::path::PathBuf;

use crate::compiler::native_agent_manifest::read_native_agent_manifest;
use crate::content::ContentRoots;
use crate::error::MarsError;
use mars_schema::agent::{AgentMode, AgentProfile};
use mars_schema::frontmatter::SkillsSpec;
use mars_schema::item::ItemKind;
use mars_schema::tool_policy::{ToolAction, ToolRule};

use super::output;

#[derive(serde::Serialize)]
struct AgentEntry {
    name: String,
    description: String,
    mode: String,
    #[serde(rename = "model-invocable")]
    model_invocable: bool,
    scope: String,
    source_path: PathBuf,
    /// Harnesses the last sync installed this agent into natively (`.mars/native-agents.json`).
    #[serde(rename = "native-harnesses")]
    native_harnesses: Vec<String>,
    /// Named in `[settings.meridian.fanout].agents`: spawned through Meridian even where it
    /// is also native.
    fanout: bool,
    /// Agents this one may spawn (the profile's `subagents`).
    subagents: Vec<String>,
}

/// `show --json`: the list entry plus the launch-profile fields only `show` reports.
#[derive(serde::Serialize)]
struct AgentDetail<'a> {
    #[serde(flatten)]
    entry: &'a AgentEntry,
    #[serde(rename = "user-invocable")]
    user_invocable: bool,
    harness: &'a str,
    model: &'a str,
    skills: Vec<String>,
    skills_structured: &'a SkillsSpec,
    approval: &'a str,
    sandbox: &'a str,
    network: &'a str,
    effort: &'a str,
    rules: &'a [ToolRule],
    /// Deprecated: derived from `rules`; cannot carry `ask`, `*` or the capability.
    tools: Vec<String>,
    #[serde(rename = "disallowed-tools")]
    disallowed_tools: Vec<String>,
}

/// Arguments for `mars agents`.
#[derive(Debug, clap::Args)]
pub struct AgentsArgs {
    /// Secondary Mars project used only for missing agents.
    #[arg(long, global = true)]
    pub fallback_root: Option<PathBuf>,

    /// Filter by mode (primary or subagent).
    ///
    /// Global so it is accepted both as `mars agents --mode ...` and on the
    /// `list` subcommand (`mars agents list --mode ...`).
    #[arg(long, global = true)]
    pub mode: Option<String>,

    /// Filter to model-invocable agents only.
    #[arg(long, global = true)]
    pub model_invocable: bool,

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
        Some(AgentsCommand::Show { name }) => {
            run_show(name, ctx, json, args.fallback_root.as_deref())
        }
        None => run_list(args, ctx, json),
    }
}

/// An agent visible from the roots, parsed, with the list entry built from it.
struct VisibleAgent {
    profile: AgentProfile,
    entry: AgentEntry,
    /// File stem, matched by `show` alongside the profile name.
    stem: String,
}

/// Agents visible from the roots whose file parses; unreadable ones are skipped with a
/// warning. `source` filters before reading, so filtering never reveals a shadowed fallback.
/// A config the bundle cannot read fails here: guessing "not fanout" would send fanout agents
/// to the harness's own tool.
fn visible_agents(
    roots: &ContentRoots,
    ctx: &super::MarsContext,
    source: Option<&str>,
) -> Result<Vec<VisibleAgent>, MarsError> {
    let visible = roots.visible_locked_items(ItemKind::Agent)?;
    output::print_warnings(&visible.warnings);
    let config = crate::build::load_effective_project_config_or_default(&ctx.project_root)?;
    let fanout = config.settings.meridian_fanout_agents();
    let native = read_native_agent_manifest(&roots.primary_mars_dir());

    let mut agents = Vec::new();
    for (scope, item, disk_path) in visible.items {
        if source.is_some_and(|source| item.source != source) {
            continue;
        }
        let content = match std::fs::read_to_string(&disk_path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", disk_path.display());
                continue;
            }
        };
        let profile = match mars_schema::read::installed_agent(&content).into_fields() {
            Ok(profile) => profile,
            Err(why) => {
                eprintln!("warning: skipping {}: {why}", disk_path.display());
                continue;
            }
        };
        let stem = path_stem(&disk_path);
        let name = profile.name.clone().unwrap_or_else(|| stem.clone());
        let entry = AgentEntry {
            native_harnesses: native.get(&name).cloned().unwrap_or_default(),
            fanout: fanout.iter().any(|f| f.eq_ignore_ascii_case(&name)),
            subagents: profile.subagents.clone(),
            description: profile.description.clone().unwrap_or_default(),
            mode: profile
                .mode
                .as_ref()
                .unwrap_or(&AgentMode::Subagent)
                .as_str()
                .to_string(),
            model_invocable: profile.model_invocable,
            scope: scope.as_str().to_string(),
            source_path: disk_path,
            name,
        };
        agents.push(VisibleAgent {
            profile,
            entry,
            stem,
        });
    }
    Ok(agents)
}

fn run_list(args: &AgentsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let roots = ContentRoots::new(&ctx.project_root, args.fallback_root.as_deref());
    let mut entries: Vec<AgentEntry> = visible_agents(&roots, ctx, args.source.as_deref())?
        .into_iter()
        .map(|agent| agent.entry)
        .filter(|e| args.mode.as_ref().is_none_or(|mode| e.mode == *mode))
        .filter(|e| !args.model_invocable || e.model_invocable)
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        output::print_json(&serde_json::json!({ "agents": entries }));
    } else {
        let rows: Vec<_> = entries
            .iter()
            .map(|e| output::CatalogRow {
                name: &e.name,
                kind: &e.mode,
                model_invocable: e.model_invocable,
                scope: &e.scope,
                description: &e.description,
            })
            .collect();
        output::print_catalog_table(&rows, "MODE", "no agents");
    }
    Ok(0)
}

fn run_show(
    name: &str,
    ctx: &super::MarsContext,
    json: bool,
    fallback_root: Option<&std::path::Path>,
) -> Result<i32, MarsError> {
    let roots = ContentRoots::new(&ctx.project_root, fallback_root);
    let found = visible_agents(&roots, ctx, None)?
        .into_iter()
        .find(|agent| {
            agent.stem.eq_ignore_ascii_case(name) || agent.entry.name.eq_ignore_ascii_case(name)
        });
    let Some(VisibleAgent { profile, entry, .. }) = found else {
        eprintln!("error: agent `{name}` not found");
        return Ok(1);
    };

    let harness = profile.harness.as_ref().map_or("", |h| h.as_str());
    let model = profile.model.as_deref().unwrap_or("");
    let approval = profile.approval.as_ref().map_or("", |a| a.as_str());
    let sandbox = profile.sandbox.as_ref().map_or("", |s| s.as_str());
    let network = profile.network.as_ref().map_or("", |n| n.as_str());
    let effort = profile.effort.as_ref().map_or("", |e| e.as_str());

    if json {
        output::print_json(&AgentDetail {
            entry: &entry,
            user_invocable: profile.user_invocable,
            harness,
            model,
            skills: profile.skills.all(),
            skills_structured: &profile.skills,
            approval,
            sandbox,
            network,
            effort,
            rules: &profile.tools.rules,
            tools: profile.tools.rendered(ToolAction::Allow),
            disallowed_tools: profile.tools.rendered(ToolAction::Deny),
        });
    } else {
        println!("name:        {}", entry.name);
        println!("description: {}", entry.description);
        println!("mode:        {}", entry.mode);
        println!("model-invocable: {}", entry.model_invocable);
        println!("user-invocable:  {}", profile.user_invocable);
        print_str_list("native-harnesses", &entry.native_harnesses);
        println!("fanout:      {}", entry.fanout);
        println!("harness:     {harness}");
        println!("model:       {model}");
        println!("approval:    {approval}");
        println!("sandbox:     {sandbox}");
        println!("network:     {network}");
        println!("effort:      {effort}");
        println!("scope:       {}", entry.scope);
        println!("source_path: {}", entry.source_path.display());
        print_str_list("skills.load", &profile.skills.load);
        print_str_list("skills.available", &profile.skills.available);
        print_str_list("subagents", &entry.subagents);
        print_str_list("tool-rules", &profile.tools.describe());
    }
    Ok(0)
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
