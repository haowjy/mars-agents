//! `mars outdated` — show available updates without applying.

use serde::Serialize;

use crate::error::MarsError;

use super::output;

/// Arguments for `mars outdated`.
#[derive(Debug, clap::Args)]
pub struct OutdatedArgs {}

/// One row in the outdated report.
#[derive(Debug, Serialize)]
struct OutdatedEntry {
    source: String,
    /// Required by another dependency, not named in `mars.toml`.
    transitive: bool,
    locked: String,
    constraint: String,
    updateable: String,
    latest: String,
}

pub fn run(_args: &OutdatedArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let lock = crate::lock::load(&ctx.project_root)?;
    let config = crate::config::load(&ctx.project_root)?;
    let local = crate::config::load_local(&ctx.project_root)?;
    let user = crate::config::load_user_config()?;
    let source_overrides = local.source_overrides(&ctx.project_root);
    let (effective, _) = crate::config::merge_with_root(config, local, &user, &ctx.project_root)?;
    let cache = crate::source::GlobalCache::new()?;

    // Resolve as sync does, so the constraints every package puts on a dependency narrow the
    // version reported here the same way they narrow `mars upgrade` and the sync hint.
    let provider = crate::resolve::RealSourceProvider::new(&cache, &ctx.project_root);
    let overridden: Vec<_> = source_overrides.keys().cloned().collect();
    let options = crate::resolve::ResolveOptions::default().with_source_overrides(source_overrides);
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let graph = crate::resolve::resolve(&effective, &provider, Some(&lock), &options, &mut diag)?;
    let mut entries = Vec::new();

    // Every dependency in the resolved graph, the ones `mars.toml` names first: the same set
    // `mars upgrade <name>` accepts and the sync hint counts.
    let names = effective
        .dependencies
        .keys()
        .chain(
            graph
                .nodes
                .keys()
                .filter(|name| !effective.dependencies.contains_key(*name)),
        )
        .filter(|name| graph.nodes.contains_key(*name));
    for name in names {
        let node = &graph.nodes[name];
        let direct = effective.dependencies.get(name);
        let transitive = direct.is_none();
        let is_overridden = overridden.contains(name);
        // An override replaces the dependency's source; a plain path dependency has no version.
        let constraint = if is_overridden {
            "-".to_string()
        } else {
            match (direct.map(|dep| &dep.spec), &node.source_id) {
                (Some(crate::config::SourceSpec::Git(git)), _) => {
                    git.version.clone().unwrap_or_else(|| "latest".to_string())
                }
                (Some(crate::config::SourceSpec::Path(_)), _) => continue,
                (None, crate::types::SourceId::Git { .. }) => transitive_constraint(&graph, name),
                (None, _) => continue,
            }
        };

        let locked_version = lock
            .dependencies
            .get(name)
            .and_then(|s| s.version.clone())
            .unwrap_or_else(|| "-".to_string());
        let unchecked = |updateable: &str, latest: &str| OutdatedEntry {
            source: name.to_string(),
            transitive,
            locked: locked_version.clone(),
            constraint: constraint.clone(),
            updateable: updateable.to_string(),
            latest: latest.to_string(),
        };

        let skipped = if is_overridden {
            Some("is overridden by mars.local.toml")
        } else if matches!(
            &node.source_id,
            crate::types::SourceId::Git {
                plugin: Some(_),
                ..
            }
        ) {
            Some("is a plugin selection")
        } else {
            None
        };
        if let Some(reason) = skipped {
            output::print_warn(&format!("`{name}` {reason}; its versions are not checked"));
            entries.push(unchecked("?", "?"));
            continue;
        }

        let checked = crate::sync::upgrades::check_upgrade(node, &graph, &provider, &options);
        let (versions, candidate) = match checked {
            Ok(Some(checked)) => (checked.available, checked.candidate),
            Ok(None) => continue,
            Err(error) => {
                output::print_warn(&format!(
                    "could not check newer versions of `{name}`: {error}"
                ));
                entries.push(unchecked("?", "?"));
                continue;
            }
        };
        let Some(fetch_url) = &node.fetch_url else {
            continue;
        };

        if versions.is_empty() {
            let current_head = crate::source::git::ls_remote_head(fetch_url.as_ref())
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
                transitive,
                locked: locked_short,
                constraint: "HEAD".to_string(),
                updateable: current_head.clone(),
                latest: current_head,
            });
            continue;
        }

        let latest = versions
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .map(|v| v.tag.clone())
            .unwrap_or_else(|| "-".to_string());

        // The version `mars upgrade` would pick; the locked one when nothing newer is allowed.
        let updateable = candidate.map_or(locked_version.clone(), |candidate| candidate.tag);
        entries.push(unchecked(&updateable, &latest));
    }

    if json {
        output::print_json(&entries);
    } else {
        print_outdated_table(&entries);
    }

    Ok(0)
}

/// What the packages that require `name` ask of its version: the ranges and pins, or `latest`.
fn transitive_constraint(
    graph: &crate::resolve::ResolvedGraph,
    name: &crate::types::SourceName,
) -> String {
    let asked: Vec<String> = graph
        .version_constraints
        .get(name)
        .into_iter()
        .flatten()
        .filter(|(_, constraint)| !matches!(constraint, mars_schema::version::Constraint::Latest))
        .map(|(_, constraint)| constraint.to_string())
        .collect();
    if asked.is_empty() {
        "latest".to_string()
    } else {
        asked.join(", ")
    }
}

fn print_outdated_table(entries: &[OutdatedEntry]) {
    if entries.is_empty() {
        output::print_info("no git dependencies to check");
        return;
    }

    let labels: Vec<String> = entries.iter().map(source_label).collect();
    let name_w = labels.iter().map(String::len).max().unwrap_or(6).max(6);
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

    for (entry, label) in entries.iter().zip(&labels) {
        println!(
            "{label:<name_w$}  {:<locked_w$}  {:<constraint_w$}  {:<update_w$}  {}",
            entry.locked, entry.constraint, entry.updateable, entry.latest
        );
    }
}

fn source_label(entry: &OutdatedEntry) -> String {
    if entry.transitive {
        format!("{} (transitive)", entry.source)
    } else {
        entry.source.clone()
    }
}
