//! Dependency resolution with semver constraints.
//!
//! Algorithm:
//! 1. Resolve package refs/versions (lock-preferred latest-compatible for git sources)
//! 2. Resolve package manifests bottom-up, recording each package's manifest entries
//! 3. Once the graph settles, select the items it installs (`select`, a pure pass)
//! 4. Emit deterministic alphabetical package order
//!
//! Uses `semver` crate for all version parsing. No custom version logic.

pub mod compat;
mod context;
mod package;
mod path;
mod plugin;
mod provider;
mod publish;
mod requires;
mod select;
mod types;
mod version;

use mars_schema::version::{Constraint, parse_constraint};
use std::collections::HashMap;
use std::path::Path;

#[cfg(test)]
use indexmap::IndexMap;

pub use context::ResolverContext;
pub(crate) use plugin::plugin_list;
pub(crate) use provider::RealSourceProvider;
pub(crate) use publish::resolve_publish_graph_from_config;
pub use publish::{PublishLens, resolve_publish_graph};
pub(crate) use requires::{
    check_consumer_package_requirements, validate_package_requirement_syntax,
};
#[cfg(test)]
pub(crate) use select::select_for_test;
pub use types::*;

pub(crate) use package::{PackageResolutionState, PendingSource, RegisteredPackage};
#[cfg(test)]
pub(crate) use path::apply_subpath;

#[cfg(test)]
use crate::config::Manifest;
use crate::config::{EffectiveConfig, SourceSpec};
use crate::diagnostic::DiagnosticCollector;
use crate::error::{MarsError, ResolutionError};
use crate::lock::LockFile;
use crate::source::{AvailableVersion, ResolvedRef};
use crate::types::SourceName;
use crate::types::SourceUrl;
use mars_schema::item::ItemKind;
use mars_schema::selection::{Requester, Selection};
use package::resolve_package_bottom_up;
use version::validate_all_constraints;

type EngineExclusions =
    HashMap<(SourceName, semver::Version), Vec<requires::EngineRequirementFailure>>;

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
#[cfg(test)]
pub trait ManifestReader {
    fn read_manifest(
        &self,
        source_tree: &Path,
        diag: &mut DiagnosticCollector,
    ) -> Result<Option<Manifest>, MarsError>;
}

/// Composite trait used by `resolve()`.
#[cfg(not(test))]
pub trait SourceProvider: VersionLister + SourceFetcher {}
#[cfg(not(test))]
impl<T> SourceProvider for T where T: VersionLister + SourceFetcher {}

/// Unit-test providers may inject manifests without writing fixture files.
#[cfg(test)]
pub trait SourceProvider: VersionLister + SourceFetcher + ManifestReader {}

#[cfg(test)]
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
                SourceSpec::Git(git) => {
                    options.direct_constraint_for(name, parse_constraint(git.version.as_deref()))
                }
                SourceSpec::Path(_) => Constraint::Latest,
            };
            reqs.push(PendingSource {
                name: name.clone(),
                declared_source_id: source.declared_source_id.clone(),
                source_id: source.source_id.clone(),
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
    // package → (correct ref, correct rooted package).
    let mut version_overrides: HashMap<
        SourceName,
        (
            ResolvedRef,
            RootedSourceRef,
            crate::staging::HookSurfaceState,
        ),
    > = HashMap::new();
    // Per-package restart history used for true oscillation detection.
    let mut restart_history: HashMap<SourceName, Vec<ResolvedRef>> = HashMap::new();
    let mut exclusions = EngineExclusions::new();

    // Restart loop: normally executes once. Restarts only when a package would
    // resolve differently under the full constraint set than it did at first-resolution
    // time (order-dependent constraint accumulation bug).
    let ctx = loop {
        let mut ctx = ResolverContext::new();
        ctx.set_version_overrides(version_overrides.clone());

        // Bottom-up phase: resolve all packages, with version selection.
        let bottom_up_result = (|| -> Result<(), MarsError> {
            for request in &direct_requests {
                resolve_package_bottom_up(
                    request,
                    provider,
                    locked,
                    options,
                    config,
                    diag,
                    &mut ctx,
                    &mut exclusions,
                )?;
            }
            Ok(())
        })();

        match bottom_up_result {
            Err(MarsError::ResolutionRestartNeeded { package }) => {
                version_overrides = ctx.version_overrides().clone();
                // Read the override info before discarding ctx.
                let Some((pkg_name, new_ref, new_rooted, hook_surface)) =
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
                version_overrides.insert(pkg_name, (new_ref, new_rooted, hook_surface));
                // Discard ctx and retry with updated overrides.
                continue;
            }
            Err(other) => return Err(other),
            Ok(()) => break ctx,
        }
    };

    let selection = select::select_registry(config, ctx.registry());
    if let Some(missing) = selection.missing_skills.first() {
        return Err(ResolutionError::SkillNotFound {
            skill: missing.skill.clone(),
            required_by: missing.required_by.clone(),
            searched: missing.searched.clone(),
            hint: missing.hint.clone(),
        }
        .into());
    }
    check_item_versions(&selection, &ctx, diag)?;

    let version_constraints = ctx.version_constraints().clone();
    let graph = ctx.into_graph(selection);

    validate_all_constraints(&graph.nodes, &version_constraints)?;
    diag.reconcile_engine_fallbacks(&graph);

    Ok(graph)
}

/// Checks each selected item's version request against the first one seen for its package.
fn check_item_versions(
    selection: &Selection,
    ctx: &ResolverContext,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut visited = VisitedSet::new();
    let mut package_versions = PackageVersions::new();
    for item in &selection.items {
        if !matches!(item.kind, ItemKind::Agent | ItemKind::Skill) {
            continue;
        }
        let Some(package) = ctx.registry().get(item.package.as_str()) else {
            continue;
        };
        let constraints = ctx.version_constraints().get(item.package.as_str());
        for requester in &item.required_by {
            // A request's constraint is the one its requester recorded; a skill ref takes the
            // package's first.
            let (required_by, recorded) = match requester {
                Requester::Consumer => ("mars.toml".to_string(), Some("mars.toml")),
                Requester::Package { package: from } => (from.clone(), Some(from.as_str())),
                Requester::Item { package, slug, .. } => (format!("{package}/{slug}"), None),
            };
            let constraint = constraints
                .and_then(|entries| match recorded {
                    Some(label) => entries.iter().find(|(from, _)| from == label),
                    None => entries.first(),
                })
                .map(|(_, constraint)| constraint)
                .unwrap_or(&package.constraint)
                .clone();
            let pending_item = PendingItem {
                package: item.package.as_str().into(),
                item: item.slug.as_str().into(),
                kind: item.kind,
                constraint,
                required_by,
                is_local: package.is_local,
            };
            match apply_item_version_policy(
                &pending_item,
                visited.check_version(
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
            package_versions
                .check_or_insert(
                    &pending_item.package,
                    &package.node.resolved_ref,
                    &pending_item.constraint,
                    &pending_item.required_by,
                    pending_item.is_local,
                )
                .map_err(MarsError::from)?;
            visited.insert(
                pending_item.package.clone(),
                pending_item.item.clone(),
                pending_item.constraint.clone(),
                package.node.resolved_ref.clone(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
fn alphabetical_order(nodes: &IndexMap<SourceName, ResolvedNode>) -> Vec<SourceName> {
    let mut order: Vec<SourceName> = nodes.keys().cloned().collect();
    order.sort();
    order
}

#[cfg(test)]
mod tests;
