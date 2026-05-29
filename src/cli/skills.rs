//! `mars skills` — list and inspect skills from the .mars/ canonical store.

use crate::compiler::skills::{parse_skill_profile, parse_skill_content};
use crate::error::MarsError;
use crate::frontmatter;
use crate::lock::ItemKind;

use super::output;

#[derive(serde::Serialize)]
struct SkillEntry {
    name: String,
    description: String,
    #[serde(rename = "type")]
    skill_type: String,
    #[serde(rename = "model-invocable")]
    model_invocable: bool,
}

/// Arguments for `mars skills`.
#[derive(Debug, clap::Args)]
pub struct SkillsArgs {
    /// Filter by skill type (e.g. guardrail, reference, principle).
    #[arg(long = "type", id = "skill_type")]
    pub skill_type: Option<String>,

    /// Filter to model-invocable skills only.
    #[arg(long)]
    pub model_invocable: bool,

    /// Filter by source name.
    #[arg(long)]
    pub source: Option<String>,

    #[command(subcommand)]
    pub command: Option<SkillsCommand>,
}

#[derive(Debug, clap::Subcommand)]
pub enum SkillsCommand {
    /// Show full metadata for a named skill.
    Show {
        /// Skill name.
        name: String,
    },
}

/// Run `mars skills`.
pub fn run(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        Some(SkillsCommand::Show { name }) => run_show(name, ctx, json),
        None => run_list(args, ctx, json),
    }
}

fn run_list(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let mars_dir = ctx.project_root.join(".mars");

    let mut entries: Vec<SkillEntry> = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Skill {
            continue;
        }

        // source filter
        if let Some(ref filter_source) = args.source
            && item.source != *filter_source
        {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let skill_md = disk_path.join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", skill_md.display());
                continue;
            }
        };

        let fm = match frontmatter::parse(&content) {
            Ok(fm) => fm,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", skill_md.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let profile = parse_skill_profile(&fm, &mut diags);

        // model_invocable filter
        if args.model_invocable && !profile.model_invocable {
            continue;
        }

        // type filter
        let type_str = profile.skill_type.clone().unwrap_or_default();
        if let Some(ref filter_type) = args.skill_type
            && type_str != *filter_type
        {
            continue;
        }

        let name = profile
            .name
            .clone()
            .unwrap_or_else(|| dir_name(&disk_path));
        let description = profile.description.clone().unwrap_or_default();

        entries.push(SkillEntry {
            name,
            description,
            skill_type: type_str,
            model_invocable: profile.model_invocable,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        output::print_json(&serde_json::json!({ "skills": entries }));
    } else {
        if entries.is_empty() {
            println!("  no skills");
        } else {
            let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(4).max(4);
            let type_w = entries
                .iter()
                .map(|e| e.skill_type.len())
                .max()
                .unwrap_or(4)
                .max(4);
            println!(
                "{:<name_w$}  {:<type_w$}  {:<5}  DESCRIPTION",
                "NAME", "TYPE", "M-INV"
            );
            for e in &entries {
                let inv = if e.model_invocable { "yes" } else { "no" };
                println!(
                    "{:<name_w$}  {:<type_w$}  {:<5}  {}",
                    e.name, e.skill_type, inv, e.description
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
        if item.kind != ItemKind::Skill {
            continue;
        }

        let disk_path = dest_path.resolve(&mars_dir);
        let skill_md = disk_path.join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", skill_md.display());
                continue;
            }
        };

        let mut diags = Vec::new();
        let (profile, _fm) = match parse_skill_content(&content, &mut diags) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", skill_md.display());
                continue;
            }
        };

        let fallback = dir_name(&disk_path);
        let skill_name = profile.name.as_deref().unwrap_or(fallback.as_str());
        if !skill_name.eq_ignore_ascii_case(name) {
            continue;
        }

        let description_str = profile.description.as_deref().unwrap_or("");
        let detail_str = profile.detail.as_deref().unwrap_or("");
        let type_str = profile.skill_type.as_deref().unwrap_or("");

        if json {
            output::print_json(&serde_json::json!({
                "name": skill_name,
                "description": description_str,
                "detail": detail_str,
                "type": type_str,
                "model-invocable": profile.model_invocable,
                "user-invocable": profile.user_invocable,
                "allowed-tools": profile.allowed_tools,
            }));
        } else {
            println!("name:          {skill_name}");
            println!("description:   {description_str}");
            println!("detail:        {detail_str}");
            println!("type:          {type_str}");
            println!("model-invocable: {}", profile.model_invocable);
            println!("user-invocable:  {}", profile.user_invocable);
            if profile.allowed_tools.is_empty() {
                println!("allowed-tools: (none)");
            } else {
                println!("allowed-tools: {}", profile.allowed_tools.join(", "));
            }
        }

        return Ok(0);
    }

    eprintln!("error: skill `{name}` not found");
    Ok(1)
}

fn dir_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}
