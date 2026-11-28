//! `mars why <name>` — explain why an item is installed.

use std::path::Path;

use serde::Serialize;

use crate::error::MarsError;
use mars_schema::item::ItemKind;

use super::output;

/// Arguments for `mars why`.
#[derive(Debug, clap::Args)]
pub struct WhyArgs {
    /// Item name to explain (e.g., "frontend-design" or "coder").
    pub name: String,
}

#[derive(Debug, Serialize)]
struct WhyResult {
    name: String,
    kind: String,
    source: String,
    version: String,
    dest_path: String,
    required_by: Vec<String>,
    /// What of the user level or a host this project left out under the same name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    yielded: Vec<serde_json::Value>,
}

pub fn run(args: &WhyArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.layout)?;

    let mut found = None;
    for (dest_path, item) in lock.canonical_flat_items() {
        let name_matches =
            dest_path.item_name(item.kind) == args.name || dest_path.as_str() == args.name;

        if name_matches {
            found = Some((dest_path, item));
            break;
        }
    }

    // What of the user level or a host this project left out under the name, and why.
    let yielded: Vec<&crate::lock::YieldedRecord> = lock
        .yielded
        .iter()
        .filter(|record| record.name == args.name)
        .collect();
    let yielded_json: Vec<serde_json::Value> = yielded
        .iter()
        .map(|record| {
            serde_json::json!({
                "kind": record.kind.to_string(),
                "target": record.target,
                "source": crate::layer::source_label(&record.source),
                "why": record.why,
            })
        })
        .collect();
    let print_yielded = || {
        for record in &yielded {
            println!(
                "  not installed here: {}: {}",
                super::list::describe_yielded(record),
                record.why
            );
        }
    };

    let (dest_path, item) = match found {
        Some(f) => f,
        None if !yielded.is_empty() => {
            if json {
                output::print_json(&serde_json::json!({
                    "name": args.name,
                    "yielded": yielded_json,
                }));
            } else {
                println!("{}", args.name);
                print_yielded();
            }
            return Ok(0);
        }
        None => {
            return Err(MarsError::Source {
                source_name: "why".to_string(),
                message: format!("item `{}` not found in lock file", args.name),
            });
        }
    };

    let mars_dir = ctx.layout.store();
    let required_by = if item.kind == ItemKind::Skill {
        let skill_name = dest_path.item_name(ItemKind::Skill);
        find_referencing_agents(mars_dir, &lock, &skill_name)
    } else {
        Vec::new()
    };

    let result = WhyResult {
        name: args.name.clone(),
        kind: item.kind.to_string(),
        source: item.source.to_string(),
        version: item.version.clone().unwrap_or_else(|| "-".to_string()),
        dest_path: dest_path.to_string(),
        required_by: required_by.clone(),
        yielded: yielded_json,
    };

    if json {
        output::print_json(&result);
    } else {
        println!("{} ({})", args.name, item.kind);
        let version = item.version.as_deref().unwrap_or("-");
        if crate::layer::is_machine_local(&item.source) {
            println!(
                "  provided by: {}, {version}",
                crate::layer::source_label(&item.source)
            );
        } else {
            println!("  provided by: {}@{version}", item.source);
        }
        println!("  installed at: {dest_path}");
        if required_by.is_empty() {
            println!("  required by: (no dependents)");
        } else {
            println!("  required by:");
            for agent in &required_by {
                println!("    {agent}");
            }
        }
        print_yielded();
    }

    Ok(0)
}

fn find_referencing_agents(
    root: &Path,
    lock: &crate::lock::LockFile,
    skill_name: &str,
) -> Vec<String> {
    let mut refs = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if item.kind != ItemKind::Agent {
            continue;
        }

        let agent_path = dest_path.resolve(root);
        if let Ok(skills) = crate::validate::parse_agent_skills(&agent_path)
            && skills.iter().any(|s| s == skill_name)
        {
            refs.push(dest_path.to_string());
        }
    }

    refs.sort();
    refs
}
