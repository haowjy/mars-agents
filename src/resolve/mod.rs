//! Dependency resolution with semver constraints.
//!
//! Algorithm:
//! 1. Resolve package refs/versions (lock-preferred latest-compatible for git sources)
//! 2. Resolve package manifests bottom-up (deps before item seeds)
//! 3. Traverse items with DFS from seeded requests and frontmatter skill deps
//! 4. Emit deterministic alphabetical package order
//!
//! Uses `semver` crate for all version parsing. No custom version logic.

pub mod compat;
mod constraint;
mod context;
mod filter;
mod package;
mod path;
mod skill;
mod types;
mod version;

use std::collections::HashMap;
use std::path::Path;

#[cfg(test)]
use indexmap::IndexMap;

pub use constraint::parse_version_constraint;
pub use context::ResolverContext;
pub use types::*;

pub(crate) use package::{PackageResolutionState, PendingSource, RegisteredPackage};
#[cfg(test)]
pub(crate) use path::apply_subpath;

use crate::config::{EffectiveConfig, Manifest, SourceSpec};
use crate::diagnostic::DiagnosticCollector;
use crate::error::{MarsError, ResolutionError};
use crate::lock::LockFile;
use crate::source::{AvailableVersion, ResolvedRef};
use crate::types::SourceName;
use crate::types::SourceUrl;
use filter::is_item_excluded;
use package::resolve_package_bottom_up;
use skill::{parse_pending_item_skill_deps, resolve_skill_ref};
use version::validate_all_constraints;

#[derive(Debug)]
enum VersionAction {
    Process,
    Skip,
}

fn apply_item_version_policy(
    pending_item: &PendingItem,
    check: VersionCheckResult,
    diag: &mut DiagnosticCollector,
) -> Result<VersionAction, ResolutionError> {
    match check {
        VersionCheckResult::NotSeen => Ok(VersionAction::Process),
        VersionCheckResult::SameVersion => Ok(VersionAction::Skip),
        VersionCheckResult::PotentiallyConflicting {
            existing,
            requested,
        } => {
            diag.warn(
                "potential-version-drift",
                format!(
                    "potential version drift: item '{}' from '{}' requested as {} but already seen as {}",
                    pending_item.item, pending_item.package, requested, existing
                ),
            );
            Ok(VersionAction::Skip)
        }
        VersionCheckResult::DifferentVersion {
            existing,
            requested,
        } => {
            if pending_item.is_local {
                return Ok(VersionAction::Skip);
            }
            Err(ResolutionError::ItemVersionConflict {
                item: pending_item.item.to_string(),
                package: pending_item.package.to_string(),
                existing: existing.to_string(),
                requested: requested.to_string(),
                chain: pending_item.required_by.clone(),
            })
        }
    }
}

fn same_resolved_ref(a: &ResolvedRef, b: &ResolvedRef) -> bool {
    a.version == b.version
        && a.version_tag == b.version_tag
        && a.commit == b.commit
        && a.tree_path == b.tree_path
}

fn describe_resolved_ref(resolved: &ResolvedRef) -> String {
    let version = resolved
        .version_tag
        .clone()
        .or_else(|| resolved.version.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| "no-version".to_string());
    let commit = resolved.commit.as_deref().unwrap_or("no-commit");
    format!("{version}@{commit}")
}

/// Lists semver-tagged versions available for a git source.
pub trait VersionLister {
    fn list_versions(&self, url: &SourceUrl) -> Result<Vec<AvailableVersion>, MarsError>;
}

/// Fetches concrete source trees after the resolver has picked a strategy.
pub trait SourceFetcher {
    /// Fetch a git source at a specific version tag.
    fn fetch_git_version(
        &self,
        url: &SourceUrl,
        version: &AvailableVersion,
        source_name: &str,
        preferred_commit: Option<&str>,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError>;

    /// Fetch a git source at a branch/commit ref (non-semver path).
    fn fetch_git_ref(
        &self,
        url: &SourceUrl,
        ref_name: &str,
        source_name: &str,
        preferred_commit: Option<&str>,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError>;

    /// Fetch a git source at an exact commit without resolving a live ref first.
    fn fetch_git_commit(
        &self,
        url: &SourceUrl,
        commit: &str,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError>;

    /// Resolve a local path source into a concrete tree reference.
    fn fetch_path(
        &self,
        path: &Path,
        source_name: &str,
        diag: &mut DiagnosticCollector,
    ) -> Result<ResolvedRef, MarsError>;
}

/// Reads source manifests for transitive dependency discovery.
pub trait ManifestReader {
    fn read_manifest(
        &self,
        source_tree: &Path,
        diag: &mut DiagnosticCollector,
    ) -> Result<Option<Manifest>, MarsError>;
}

/// Composite trait used by `resolve()`.
pub trait SourceProvider: VersionLister + SourceFetcher + ManifestReader {}

impl<T> SourceProvider for T where T: VersionLister + SourceFetcher + ManifestReader {}

/// Resolve the full dependency graph from config.
///
/// Uses lock-preferred latest-compatible selection by default: if the lock has
/// a compatible version, replay it; otherwise pick the newest satisfying version.
/// Users who want lock-agnostic maximization use `mars upgrade`.
///
/// When `locked` is provided, prefer locked versions when constraints allow
/// (reproducible builds).
///
/// ## Fresh-context restart algorithm
///
/// The bottom-up traversal can discover that an already-resolved package would
/// select a different version under the full accumulated constraint set (e.g.
/// a `Latest` constraint from a later-processed package changes the optimum).
/// When this happens `resolve_package_bottom_up` emits `ResolutionRestartNeeded`.
///
/// The driver handles this by:
///   1. Reading the "correct" (new) ref from the context.
///   2. Carrying it as an override into a fresh `ResolverContext`.
///   3. Restarting the bottom-up phase from scratch.
///
/// On the next pass the override is used at first-resolution time — the package
/// starts at the right version, so the same constraint pattern does NOT re-trigger
/// a restart. B1 (stale manifest-derived constraints) and B2 (new deps not
/// materialized) are avoided by construction because the fresh context has no stale
/// state and the override falls through to the normal first-resolution code path.
///
/// Convergence is guaranteed in practice because versions only move in one direction
/// (upward under maximize, toward the lock-preferred/latest-compatible optimum).
/// If a package starts bouncing between previously-seen refs, the driver reports
/// a true per-package oscillation with the observed ref cycle.
pub fn resolve(
    config: &EffectiveConfig,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedGraph, MarsError> {
    // Build direct requests (stable across restarts — determined by config + options).
    let direct_requests: Vec<PendingSource> = {
        let mut reqs = Vec::new();
        for (name, source) in &config.dependencies {
            let constraint = match &source.spec {
                SourceSpec::Git(git) => options
                    .direct_constraint_for(name, parse_version_constraint(git.version.as_deref())),
                SourceSpec::Path(_) => VersionConstraint::Latest,
            };
            reqs.push(PendingSource {
                name: name.clone(),
                source_id: source.id.clone(),
                spec: source.spec.clone(),
                subpath: source.subpath.clone(),
                constraint,
                filter: source.filter.clone(),
                required_by: "mars.toml".to_string(),
            });
        }
        reqs
    };

    // Version overrides carried across restarts:
    // package → (correct ref, correct rooted, latest_version metadata).
    let mut version_overrides: HashMap<
        SourceName,
        (ResolvedRef, RootedSourceRef, Option<semver::Version>),
    > = HashMap::new();
    // Per-package restart history used for true oscillation detection.
    let mut restart_history: HashMap<SourceName, Vec<ResolvedRef>> = HashMap::new();

    // Restart loop: normally executes once. Restarts only when a package would
    // resolve differently under the full constraint set than it did at first-resolution
    // time (order-dependent constraint accumulation bug).
    let ctx = loop {
        let mut ctx = ResolverContext::new();
        ctx.set_version_overrides(version_overrides.clone());

        // Bottom-up phase: resolve all packages (with version selection) and seed items.
        let bottom_up_result = (|| -> Result<(), MarsError> {
            for request in direct_requests
                .iter()
                .filter(|request| filter::is_unfiltered_request(&request.filter))
            {
                resolve_package_bottom_up(
                    request, true, provider, locked, options, diag, &mut ctx,
                )?;
            }
            for request in direct_requests
                .iter()
                .filter(|request| !filter::is_unfiltered_request(&request.filter))
            {
                resolve_package_bottom_up(
                    request, true, provider, locked, options, diag, &mut ctx,
                )?;
            }
            Ok(())
        })();

        match bottom_up_result {
            Err(MarsError::ResolutionRestartNeeded { package }) => {
                // Read the override info before discarding ctx.
                let Some((pkg_name, new_ref, new_rooted, latest_version)) =
                    ctx.take_pending_restart()
                else {
                    return Err(MarsError::Internal(format!(
                        "missing pending restart payload for `{package}`"
                    )));
                };
                let history = restart_history.entry(pkg_name.clone()).or_default();
                if let Some(cycle_start) = history
                    .iter()
                    .position(|seen| same_resolved_ref(seen, &new_ref))
                {
                    let mut cycle: Vec<String> = history[cycle_start..]
                        .iter()
                        .map(describe_resolved_ref)
                        .collect();
                    cycle.push(describe_resolved_ref(&new_ref));
                    return Err(MarsError::Resolution(ResolutionError::VersionConflict {
                        name: pkg_name.to_string(),
                        message: format!(
                            "resolution oscillation detected for `{pkg_name}`: {}",
                            cycle.join(" -> ")
                        ),
                    }));
                }
                history.push(new_ref.clone());
                version_overrides.insert(pkg_name, (new_ref, new_rooted, latest_version));
                // Discard ctx and retry with updated overrides.
                continue;
            }
            Err(other) => return Err(other),
            Ok(()) => break ctx,
        }
    };

    // Item DFS phase: traverse seeded items, resolve skill deps.
    let mut ctx = ctx;
    while let Some(pending_item) = ctx.pop_pending() {
        let (resolved_ref, skill_deps) = {
            let Some(package) = ctx.registry().get(&pending_item.package) else {
                return Err(ResolutionError::SourceNotFound {
                    name: pending_item.package.to_string(),
                }
                .into());
            };

            if package
                .item(pending_item.kind, &pending_item.item)
                .is_none()
            {
                continue;
            }

            let skill_deps = parse_pending_item_skill_deps(&pending_item, package)?;
            (package.node.resolved_ref.clone(), skill_deps)
        };

        match apply_item_version_policy(
            &pending_item,
            ctx.visited().check_version(
                &pending_item.package,
                &pending_item.item,
                &pending_item.constraint,
            ),
            diag,
        )
        .map_err(MarsError::from)?
        {
            VersionAction::Process => {}
            VersionAction::Skip => continue,
        }

        ctx.package_versions_mut()
            .check_or_insert(
                &pending_item.package,
                &resolved_ref,
                &pending_item.constraint,
                &pending_item.required_by,
                pending_item.is_local,
            )
            .map_err(MarsError::from)?;

        ctx.visited_mut().insert(
            pending_item.package.clone(),
            pending_item.item.clone(),
            pending_item.constraint.clone(),
            resolved_ref,
        );

        for skill_dep in skill_deps {
            let resolved_skill = resolve_skill_ref(
                &skill_dep,
                &pending_item,
                ctx.registry(),
                ctx.version_constraints(),
            )?;
            if is_item_excluded(
                ctx.materialization_filters(),
                ctx.registry(),
                &resolved_skill.package,
                resolved_skill.kind,
                &resolved_skill.item,
            ) {
                continue;
            }
            ctx.add_filter(
                &resolved_skill.package,
                crate::config::FilterMode::Include {
                    agents: Vec::new(),
                    skills: vec![resolved_skill.item.clone()],
                },
            );
            ctx.push_pending(resolved_skill);
        }
    }

    let version_constraints = ctx.version_constraints().clone();
    let graph = ctx.into_graph();

    validate_all_constraints(&graph.nodes, &version_constraints)?;

    Ok(graph)
}

#[cfg(test)]
fn alphabetical_order(nodes: &IndexMap<SourceName, ResolvedNode>) -> Vec<SourceName> {
    let mut order: Vec<SourceName> = nodes.keys().cloned().collect();
    order.sort();
    order
}

#[cfg(test)]
mod tests;
