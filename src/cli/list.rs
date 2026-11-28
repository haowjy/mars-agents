//! `mars list` — show available agents and skills.

use std::path::Path;

use crate::error::MarsError;
use crate::hash;
use mars_schema::item::ItemKind;

use super::output::{self, CatalogEntry, ListEntry};

/// Arguments for `mars list`.
#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Filter by source name.
    #[arg(long)]
    pub source: Option<String>,

    /// Filter by item kind (agents, skills).
    #[arg(long)]
    pub kind: Option<String>,

    /// Show detailed status (source, version, hash check).
    #[arg(long)]
    pub status: bool,
}

pub fn run(args: &ListArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.layout)?;

    let mars_dir = ctx.layout.store();
    if args.status {
        return run_status(args, mars_dir, &lock, json);
    }

    let mut agents = Vec::new();
    let mut skills = Vec::new();
    let mut bootstrap = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if let Some(ref filter_source) = args.source
            && item.source != *filter_source
        {
            continue;
        }

        if let Some(ref filter_kind) = args.kind {
            let kind_str = match item.kind {
                ItemKind::Agent => "agents",
                ItemKind::Skill => "skills",
                ItemKind::Hook => "hooks",
                ItemKind::McpServer => "mcp",
                ItemKind::BootstrapDoc => "bootstrap",
            };
            if kind_str != filter_kind && &item.kind.to_string() != filter_kind {
                continue;
            }
        }

        let disk_path = dest_path.resolve(mars_dir);
        let content_path = match item.kind {
            ItemKind::Agent | ItemKind::Hook | ItemKind::McpServer | ItemKind::BootstrapDoc => {
                disk_path.clone()
            }
            ItemKind::Skill => disk_path.join("SKILL.md"),
        };

        let fallback_name = match item.kind {
            ItemKind::BootstrapDoc => dest_path.item_name(item.kind),
            _ => path_to_name(&disk_path),
        };
        let (name, description) = read_name_description(&content_path, &fallback_name, item.kind);
        let variants = match item.kind {
            ItemKind::Skill => {
                let (index, _warnings) =
                    crate::compiler::variants::index_skill_variants(&disk_path);
                index.annotation()
            }
            _ => None,
        };

        let entry = CatalogEntry {
            name,
            description,
            kind: item.kind.to_string(),
            variants,
        };

        match item.kind {
            ItemKind::Agent => agents.push(entry),
            ItemKind::Skill => skills.push(entry),
            ItemKind::BootstrapDoc => bootstrap.push(entry),
            ItemKind::Hook | ItemKind::McpServer => {}
        }
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    bootstrap.sort_by(|a, b| a.name.cmp(&b.name));

    // What of the user level or a host this project left out, and why: nothing else shows it.
    let yielded: Vec<&crate::lock::YieldedRecord> = lock
        .yielded
        .iter()
        .filter(|record| {
            args.source
                .as_deref()
                .is_none_or(|source| record.source == *source)
        })
        .filter(|record| {
            args.kind.as_deref().is_none_or(|kind| {
                kind == record.kind.to_string() || kind == kind_plural(record.kind)
            })
        })
        .collect();

    if json {
        output::print_json(&serde_json::json!({
            "agents": agents,
            "skills": skills,
            "bootstrap": bootstrap,
            "yielded": yielded
                .iter()
                .map(|record| serde_json::json!({
                    "name": record.name,
                    "kind": record.kind.to_string(),
                    "target": record.target,
                    "source": crate::layer::source_label(&record.source),
                    "why": record.why,
                }))
                .collect::<Vec<_>>(),
        }));
    } else {
        output::print_catalog(&agents, &skills, &bootstrap, args.kind.as_deref());
        if !yielded.is_empty() {
            println!();
            println!("NOT INSTALLED HERE");
            for record in yielded {
                println!("- {}: {}", describe_yielded(record), record.why);
            }
        }
    }

    Ok(0)
}

fn kind_plural(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Agent => "agents",
        ItemKind::Skill => "skills",
        ItemKind::Hook => "hooks",
        ItemKind::McpServer => "mcp",
        ItemKind::BootstrapDoc => "bootstrap",
    }
}

/// `skill foo from upkg (user level)`, with the target for a hook or MCP server.
pub(super) fn describe_yielded(record: &crate::lock::YieldedRecord) -> String {
    let on = if record.target == crate::lock::CANONICAL_TARGET_ROOT {
        String::new()
    } else {
        format!(" on `{}`", record.target)
    };
    format!(
        "{} `{}`{on} from {}",
        record.kind,
        record.name,
        crate::layer::source_label(&record.source)
    )
}

fn read_name_description(path: &Path, fallback_name: &str, kind: ItemKind) -> (String, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (fallback_name.to_string(), String::new()),
    };
    let fields = if kind == ItemKind::Skill {
        mars_schema::skill::read(&content)
            .fields
            .map(|profile| (profile.name, profile.description))
    } else {
        mars_schema::agent::read(&content)
            .fields
            .map(|profile| (profile.name, profile.description))
    };
    fields.map_or_else(
        || (fallback_name.to_string(), String::new()),
        |(name, description)| {
            (
                name.unwrap_or_else(|| fallback_name.to_string()),
                description.unwrap_or_default(),
            )
        },
    )
}

fn path_to_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.parent().and_then(|p| p.file_name()))
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn run_status(
    args: &ListArgs,
    root: &Path,
    lock: &crate::lock::LockFile,
    json: bool,
) -> Result<i32, MarsError> {
    let mut entries = Vec::new();

    for (dest_path, item) in lock.canonical_flat_items() {
        if let Some(ref filter_source) = args.source
            && item.source != *filter_source
        {
            continue;
        }

        if let Some(ref filter_kind) = args.kind {
            let kind_str = match item.kind {
                ItemKind::Agent => "agents",
                ItemKind::Skill => "skills",
                ItemKind::Hook => "hooks",
                ItemKind::McpServer => "mcp",
                ItemKind::BootstrapDoc => "bootstrap",
            };
            if kind_str != filter_kind && &item.kind.to_string() != filter_kind {
                continue;
            }
        }

        let disk_path = dest_path.resolve(root);
        let status = if !disk_path.exists() {
            "missing".to_string()
        } else if super::conflict_markers::file_has_conflict_markers(&disk_path) {
            "conflicted".to_string()
        } else {
            let disk_hash = hash::compute_hash(&disk_path, item.kind)?;
            if disk_hash == item.installed_checksum.as_ref() {
                "ok".to_string()
            } else {
                "modified".to_string()
            }
        };

        entries.push(ListEntry {
            source: item.source.to_string(),
            item: dest_path.to_string(),
            kind: item.kind.to_string(),
            version: item.version.clone().unwrap_or_else(|| "-".to_string()),
            status,
        });
    }

    entries.sort_by(|a, b| (&a.source, &a.item).cmp(&(&b.source, &b.item)));
    output::print_list(&entries, json);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_name_description_uses_provided_fallback_name() {
        let dir = TempDir::new().unwrap();
        let missing_skill_md = dir.path().join("skills/test-skill/SKILL.md");
        let (name, description) =
            read_name_description(&missing_skill_md, "test-skill", ItemKind::Skill);
        assert_eq!(name, "test-skill");
        assert_eq!(description, "");
    }
}
