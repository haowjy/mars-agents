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
    locked: String,
    constraint: String,
    updateable: String,
    latest: String,
}

/// Run `mars outdated`.
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

    for (name, dep) in &effective.dependencies {
        let Some(node) = graph.nodes.get(name) else {
            continue;
        };
        let is_overridden = overridden.contains(name);
        let git_version = match &dep.spec {
            crate::config::SourceSpec::Git(git) => git.version.clone(),
            // An override replaces the dependency's source; a plain path dependency has no version.
            crate::config::SourceSpec::Path(_) if is_overridden => None,
            crate::config::SourceSpec::Path(_) => continue,
        };

        let locked_version = lock
            .dependencies
            .get(name)
            .and_then(|s| s.version.clone())
            .unwrap_or_else(|| "-".to_string());
        let constraint = match (&git_version, is_overridden) {
            (_, true) => "-".to_string(),
            (version, false) => version.clone().unwrap_or_else(|| "latest".to_string()),
        };
        let unchecked = |updateable: &str, latest: &str| OutdatedEntry {
            source: name.to_string(),
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

        let checked = crate::sync::upgrades::check_upgrade(
            node,
            &graph,
            &provider,
            &options,
            &mut crate::diagnostic::DiagnosticCollector::new(),
        );
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
        let crate::types::SourceId::Git { url, .. } = &node.source_id else {
            continue;
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
