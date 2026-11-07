//! `mars skills` — list and inspect skills from the .mars/ canonical store.

use std::path::PathBuf;

use crate::content::ContentRoots;
use crate::error::MarsError;
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

pub fn run(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    match &args.command {
        Some(SkillsCommand::List) => run_list(args, ctx, json),
        Some(SkillsCommand::Show { name }) => {
            run_show(name, ctx, json, args.fallback_root.as_deref())
        }
        None => run_list(args, ctx, json),
    }
}

/// A skill visible from the roots, parsed, with its `SKILL.md` path.
struct VisibleSkill {
    scope: String,
    skill_md: PathBuf,
    /// Directory name, matched by `show` alongside the profile name.
    dir: String,
    name: String,
    profile: mars_schema::skill::SkillProfile,
}

/// Skills visible from the roots whose `SKILL.md` parses; unreadable ones are skipped with a
/// warning. `source` filters before reading, so filtering never reveals a shadowed fallback.
fn visible_skills(
    roots: &ContentRoots,
    source: Option<&str>,
) -> Result<Vec<VisibleSkill>, MarsError> {
    let visible = roots.visible_locked_items(ItemKind::Skill)?;
    output::print_warnings(&visible.warnings);

    let mut skills = Vec::new();
    for (scope, item, disk_path) in visible.items {
        if source.is_some_and(|source| item.source != source) {
            continue;
        }
        let skill_md = disk_path.join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_md) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("warning: skipping {}: {err}", skill_md.display());
                continue;
            }
        };
        let profile = match mars_schema::read::installed_skill(&content).into_fields() {
            Ok(profile) => profile,
            Err(why) => {
                eprintln!("warning: skipping {}: {why}", skill_md.display());
                continue;
            }
        };
        skills.push(VisibleSkill {
            scope: scope.as_str().to_string(),
            name: profile.name.clone().unwrap_or_else(|| dir_name(&disk_path)),
            dir: dir_name(&disk_path),
            skill_md,
            profile,
        });
    }
    Ok(skills)
}

impl VisibleSkill {
    fn entry(&self) -> SkillEntry {
        SkillEntry {
            name: self.name.clone(),
            description: self.profile.description.clone().unwrap_or_default(),
            skill_type: self.profile.skill_type.clone().unwrap_or_default(),
            model_invocable: self.profile.model_invocable,
            scope: self.scope.clone(),
            source_path: self.skill_md.clone(),
        }
    }
}

/// `show --json`: the list entry plus the fields only `show` reports.
#[derive(serde::Serialize)]
struct SkillDetail {
    #[serde(flatten)]
    entry: SkillEntry,
    #[serde(rename = "user-invocable")]
    user_invocable: bool,
}

fn run_list(args: &SkillsArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let roots = ContentRoots::new(&ctx.project_root, args.fallback_root.as_deref());
    let mut entries: Vec<SkillEntry> = visible_skills(&roots, args.source.as_deref())?
        .iter()
        .filter(|skill| !args.model_invocable || skill.profile.model_invocable)
        .map(VisibleSkill::entry)
        .filter(|entry| {
            args.skill_type
                .as_ref()
                .is_none_or(|filter| entry.skill_type == *filter)
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if json {
        output::print_json(&serde_json::json!({ "skills": entries }));
    } else {
        let rows: Vec<_> = entries
            .iter()
            .map(|e| output::CatalogRow {
                name: &e.name,
                kind: &e.skill_type,
                model_invocable: e.model_invocable,
                scope: &e.scope,
                description: &e.description,
            })
            .collect();
        output::print_catalog_table(&rows, "TYPE", "no skills");
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
    let found = visible_skills(&roots, None)?.into_iter().find(|skill| {
        skill.name.eq_ignore_ascii_case(name) || skill.dir.eq_ignore_ascii_case(name)
    });
    let Some(skill) = found else {
        eprintln!("error: skill `{name}` not found");
        return Ok(1);
    };

    let entry = skill.entry();
    if json {
        output::print_json(&SkillDetail {
            entry,
            user_invocable: skill.profile.user_invocable,
        });
    } else {
        println!("name:          {}", entry.name);
        println!("description:   {}", entry.description);
        println!("type:          {}", entry.skill_type);
        println!("model-invocable: {}", entry.model_invocable);
        println!("user-invocable:  {}", skill.profile.user_invocable);
        println!("scope:         {}", entry.scope);
        println!("source_path:   {}", entry.source_path.display());
    }
    Ok(0)
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
