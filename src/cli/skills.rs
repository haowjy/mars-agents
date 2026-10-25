//! `mars skills` — list and inspect skills from the .mars/ canonical store.

use std::path::PathBuf;

use crate::compiler::skills::{parse_skill_content, parse_skill_profile};
use crate::content::ContentRoots;
use crate::error::MarsError;
use mars_schema::frontmatter;
use mars_schema::item::ItemKind;

use super::output;

#[derive(serde::Serialize)]
struct SkillEntry {
    name: String,
    description: String,
    #[serde(rename = "type")]
    skill_type: String,
    #[serde(rename = "model-invocable")]
    model_invocable: bool,
    scope: String,
    source_path: PathBuf,
}

/// Arguments for `mars skills`.
#[derive(Debug, clap::Args)]
pub struct SkillsArgs {
    /// Secondary Mars project used only for missing skills.
    #[arg(long, global = true)]
    pub fallback_root: Option<PathBuf>,

    /// Filter by skill type (e.g. guardrail, reference, principle).
    ///
    /// Global so it is accepted both as `mars skills --type ...` and on the
    /// `list` subcommand (`mars skills list --type ...`).
    #[arg(long = "type", id = "skill_type", global = true)]
    pub skill_type: Option<String>,

    /// Filter to model-invocable skills only.
    #[arg(long, global = true)]
    pub model_invocable: bool,

    /// Filter by source name.
    #[arg(long, global = true)]
    pub source: Option<String>,

    #[command(subcommand)]
    pub command: Option<SkillsCommand>,
}

#[derive(Debug, clap::Subcommand)]
pub enum SkillsCommand {
    /// List all skills (same as bare `mars skills`).
    List,
    /// Show full metadata for a named skill.
    Show {
        /// Skill name.
        name: String,
    },
}

/// Run `mars skills`.
pub fn run(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        Some(SkillsCommand::List) => run_list(args, ctx, json),
        Some(SkillsCommand::Show { name }) => {
            run_show(name, ctx, json, args.fallback_root.as_deref())
        }
        None => run_list(args, ctx, json),
    }
}

fn run_list(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let mut entries: Vec<SkillEntry> = Vec::new();
    let roots = ContentRoots::new(&ctx.project_root, args.fallback_root.as_deref());
    let visible = roots.visible_locked_items(ItemKind::Skill)?;
    output::print_warnings(&visible.warnings);

    for (scope, item, disk_path) in visible.items {
        let skill_md = disk_path.join("SKILL.md");

        // Filtering never reveals a shadowed fallback item.
        if let Some(ref filter_source) = args.source
            && item.source != *filter_source
        {
            continue;
        }

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

        if args.model_invocable && !profile.model_invocable {
            continue;
        }

        let type_str = profile.skill_type.clone().unwrap_or_default();
        if let Some(ref filter_type) = args.skill_type
            && type_str != *filter_type
        {
            continue;
        }

        let name = profile.name.clone().unwrap_or_else(|| dir_name(&disk_path));
        let description = profile.description.clone().unwrap_or_default();

        entries.push(SkillEntry {
            name,
            description,
            skill_type: type_str,
            model_invocable: profile.model_invocable,
            scope: scope.as_str().to_string(),
            source_path: skill_md,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        output::print_json(&serde_json::json!({ "skills": entries }));
    } else {
        if entries.is_empty() {
            println!("  no skills");
        } else {
            let name_w = entries
                .iter()
                .map(|e| e.name.len())
                .max()
                .unwrap_or(4)
                .max(4);
            let type_w = entries
                .iter()
                .map(|e| e.skill_type.len())
                .max()
                .unwrap_or(4)
                .max(4);
            let scope_w = entries
                .iter()
                .map(|e| e.scope.len())
                .max()
                .unwrap_or(5)
                .max(5);
            println!(
                "{:<name_w$}  {:<type_w$}  {:<5}  {:<scope_w$}  DESCRIPTION",
                "NAME", "TYPE", "M-INV", "SCOPE"
            );
            for e in &entries {
                let inv = if e.model_invocable { "yes" } else { "no" };
                println!(
                    "{:<name_w$}  {:<type_w$}  {:<5}  {:<scope_w$}  {}",
                    e.name, e.skill_type, inv, e.scope, e.description
                );
            }
        }
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
    let visible = roots.visible_locked_items(ItemKind::Skill)?;
    output::print_warnings(&visible.warnings);

    for (scope, _item, disk_path) in visible.items {
        let skill_md = disk_path.join("SKILL.md");
        let fallback = dir_name(&disk_path);

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

        let skill_name = profile.name.as_deref().unwrap_or(fallback.as_str());
        if !fallback.eq_ignore_ascii_case(name) && !skill_name.eq_ignore_ascii_case(name) {
            continue;
        }

        let description_str = profile.description.as_deref().unwrap_or("");
        let type_str = profile.skill_type.as_deref().unwrap_or("");
        let scope = scope.as_str();

        if json {
            output::print_json(&serde_json::json!({
                "name": skill_name,
                "description": description_str,
                "type": type_str,
                "model-invocable": profile.model_invocable,
                "user-invocable": profile.user_invocable,
                "scope": scope,
                "source_path": skill_md,
            }));
        } else {
            println!("name:          {skill_name}");
            println!("description:   {description_str}");
            println!("type:          {type_str}");
            println!("model-invocable: {}", profile.model_invocable);
            println!("user-invocable:  {}", profile.user_invocable);
            println!("scope:         {scope}");
            println!("source_path:   {}", skill_md.display());
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

#[cfg(test)]
mod filter_flag_tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;

    fn skills_args(args: &[&str]) -> super::SkillsArgs {
        match Cli::try_parse_from(args).expect("should parse").command {
            Command::Skills(s) => s,
            other => panic!("expected skills command, got {other:?}"),
        }
    }

    #[test]
    fn type_filter_populates_on_both_bare_and_list_forms() {
        for args in [
            ["mars", "skills", "list", "--type", "guardrail"].as_slice(),
            ["mars", "skills", "--type", "guardrail", "list"].as_slice(),
            ["mars", "skills", "--type", "guardrail"].as_slice(),
        ] {
            let parsed = skills_args(args);
            assert_eq!(
                parsed.skill_type.as_deref(),
                Some("guardrail"),
                "args: {args:?}"
            );
        }
    }

    #[test]
    fn model_invocable_flag_populates_on_list_form() {
        let parsed = skills_args(&["mars", "skills", "list", "--model-invocable"]);
        assert!(parsed.model_invocable);
    }
}
