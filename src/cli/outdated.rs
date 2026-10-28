//! `mars outdated` — show available updates without applying.

use serde::Serialize;

use crate::error::MarsError;
use crate::types::SourceUrl;

use super::output;

/// Arguments for `mars outdated`.
#[derive(Debug, clap::Args)]
pub struct OutdatedArgs {}

/// One row in the outdated report.
#[derive(Debug, Serialize)]
struct OutdatedEntry {
    source: String,
    locked: String,
    constraint: String,
    updateable: String,
    latest: String,
}

/// Run `mars outdated`.
pub fn run(_args: &OutdatedArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let config = crate::config::load(&ctx.project_root)?;
    let cache = crate::source::GlobalCache::new()?;

    let provider = crate::resolve::RealSourceProvider::new(&cache, &ctx.project_root);
    let mut entries = Vec::new();

    for (name, dep_entry) in &config.dependencies {
        // Only check git dependencies with versions
        let Some(url) = &dep_entry.url else {
            continue; // local path deps have no version
        };
        let url = &SourceUrl::from(url.as_str());

        let locked_version = lock
            .dependencies
            .get(name)
            .and_then(|s| s.version.clone())
            .unwrap_or_else(|| "-".to_string());

        let constraint = dep_entry
            .version
            .clone()
            .unwrap_or_else(|| "latest".to_string());

        // Try to list versions (may fail for non-git sources)
        let versions = match crate::source::list_versions(url, &cache) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if versions.is_empty() {
            // Untagged repo — compare locked commit vs current HEAD
            let current_head = crate::source::git::ls_remote_head(url.as_ref())
                .map(|sha| {
                    if sha.len() >= 12 {
                        sha[..12].to_string()
                    } else {
                        sha
                    }
                })
                .unwrap_or_else(|_| "-".to_string());
            let locked_commit = lock
                .dependencies
                .get(name)
                .and_then(|s| s.commit.as_ref().map(|c| c.to_string()))
                .unwrap_or_else(|| "-".to_string());
            let locked_short = if locked_commit.len() >= 12 {
                locked_commit[..12].to_string()
            } else {
                locked_commit
            };
            entries.push(OutdatedEntry {
                source: name.to_string(),
                locked: locked_short,
                constraint: "HEAD".to_string(),
                updateable: current_head.clone(),
                latest: current_head,
            });
            continue;
        }

        // Find latest version overall
        let latest = versions
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .map(|v| v.tag.clone())
            .unwrap_or_else(|| "-".to_string());

        // The version `mars upgrade` would pick: the newest the constraint allows whose
        // `requires-*` this Mars meets.
        let parsed_constraint =
            mars_schema::version::parse_constraint(dep_entry.version.as_deref());
        let locked_floor = mars_schema::version::parse_tag(&locked_version);
        let updateable = match crate::resolve::upgrade_candidate(
            name,
            url,
            dep_entry.subpath.as_ref(),
            &versions,
            locked_floor.as_ref(),
            &[("mars.toml", &parsed_constraint)],
            &provider,
            &crate::resolve::ResolveOptions::default(),
            &mut crate::diagnostic::DiagnosticCollector::new(),
        ) {
            Ok(Some(candidate)) => candidate.tag.clone(),
            Ok(None) => locked_version.clone(),
            Err(error) => {
                output::print_warn(&format!(
                    "could not check newer versions of `{name}`: {error}"
                ));
                "?".to_string()
            }
        };

        entries.push(OutdatedEntry {
            source: name.to_string(),
            locked: locked_version,
            constraint,
            updateable,
            latest,
        });
    }

    if json {
        output::print_json(&entries);
    } else {
        print_outdated_table(&entries);
    }

    Ok(0)
}

fn print_outdated_table(entries: &[OutdatedEntry]) {
    if entries.is_empty() {
        output::print_info("no git dependencies to check");
        return;
    }

    let name_w = entries
        .iter()
        .map(|e| e.source.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let locked_w = entries
        .iter()
        .map(|e| e.locked.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let constraint_w = entries
        .iter()
        .map(|e| e.constraint.len())
        .max()
        .unwrap_or(10)
        .max(10);
    let update_w = entries
        .iter()
        .map(|e| e.updateable.len())
        .max()
        .unwrap_or(10)
        .max(10);

    println!(
        "{:<name_w$}  {:<locked_w$}  {:<constraint_w$}  {:<update_w$}  LATEST",
        "SOURCE", "LOCKED", "CONSTRAINT", "UPDATEABLE"
    );

    for entry in entries {
        println!(
            "{:<name_w$}  {:<locked_w$}  {:<constraint_w$}  {:<update_w$}  {}",
            entry.source, entry.locked, entry.constraint, entry.updateable, entry.latest
        );
    }
}
