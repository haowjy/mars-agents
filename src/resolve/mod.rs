//! Dependency resolution with semver constraints.
//!
//! Algorithm:
//! 1. Resolve package refs/versions (lock-preferred latest-compatible for git sources)
//! 2. Resolve package manifests bottom-up, recording each package's manifest entries
//! 3. Once the graph settles, select the items it installs (`select`, a pure pass)
//! 4. Emit deterministic alphabetical package order
//!
//! Uses `semver` crate for all version parsing. No custom version logic.

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

pub use context::ResolverContext;
pub(crate) use plugin::plugin_list;
pub(crate) use provider::RealSourceProvider;
pub use publish::PublishLens;
pub(crate) use publish::resolve_publish_graph_from_config;
pub(crate) use requires::{
    check_consumer_package_requirements, validate_package_requirement_syntax,
};
#[cfg(test)]
pub(crate) use select::select_for_test;
pub(crate) use select::{PUBLISHED_PACKAGE, select_published};
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
use package::resolve_package_bottom_up;
use version::validate_all_constraints;

type EngineExclusions =
    HashMap<(SourceName, semver::Version), Vec<requires::EngineRequirementFailure>>;

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
    for diagnostic in &selection.diagnostics {
        diag.schema(diagnostic, "selection", false);
    }
    if let Some(missing) = selection.missing_skills.first() {
        return Err(ResolutionError::SkillNotFound {
            skill: missing.skill.clone(),
            required_by: missing.required_by.clone(),
            searched: missing.searched.clone(),
            hint: missing.hint.clone(),
        }
        .into());
    }
    {
        let settled: HashMap<SourceName, version::SettledPackage> = ctx
            .registry()
            .iter()
            .map(|(name, package)| {
                (
                    name.clone(),
                    version::SettledPackage {
                        version: package.node.resolved_ref.version.as_ref(),
                        tag: package.node.resolved_ref.version_tag.as_deref(),
                        is_local: package.is_local,
                    },
                )
            })
            .collect();
        version::check_selecting_requests_agree(
            &selection,
            ctx.version_constraints(),
            &settled,
            diag,
        )?;
    }

    let version_constraints = ctx.version_constraints().clone();
    let graph = ctx.into_graph(selection);

    validate_all_constraints(&graph.nodes, &version_constraints)?;
    diag.reconcile_engine_fallbacks(&graph);

    Ok(graph)
}

#[cfg(test)]
mod tests;
