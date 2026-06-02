//! Upgrade hint calculation for sync reports.

use semver::{Version, VersionReq};

use crate::diagnostic::DiagnosticCollector;
use crate::resolve::{ResolvedGraph, SourceProvider, VersionConstraint};
use crate::source::AvailableVersion;
use crate::types::{SourceId, SourceName, SourceUrl};

pub(crate) fn count_compatible_upgrades(
    graph: &ResolvedGraph,
    provider: &dyn SourceProvider,
    diag: &mut DiagnosticCollector,
) -> usize {
    graph
        .nodes
        .values()
        .filter(|node| {
            let Some(resolved) = node.resolved_ref.version.as_ref() else {
                return false;
            };
            let SourceId::Git { url, .. } = &node.source_id else {
                return false;
            };
            let Some(constraints) = graph.version_constraints.get(&node.source_name) else {
                return false;
            };
            has_newer_compatible_version(
                &node.source_name,
                url,
                resolved,
                constraints,
                provider,
                diag,
            )
        })
        .count()
}

fn has_newer_compatible_version(
    name: &SourceName,
    url: &SourceUrl,
    resolved: &Version,
    constraints: &[(String, VersionConstraint)],
    provider: &dyn SourceProvider,
    diag: &mut DiagnosticCollector,
) -> bool {
    let mut semver_reqs: Vec<&VersionReq> = Vec::new();
    for (_, constraint) in constraints {
        match constraint {
            VersionConstraint::Semver(req) => semver_reqs.push(req),
            VersionConstraint::Latest => {}
            VersionConstraint::RefPin(_) => return false,
        }
    }

    match provider.list_versions(url) {
        Ok(available) => latest_compatible_version(&available, &semver_reqs)
            .is_some_and(|latest| latest > *resolved),
        Err(err) => {
            diag.warn(
                "upgrade-hint-unavailable",
                format!(
                    "could not list current versions for upgrade hint on `{name}` ({url}): {err}"
                ),
            );
            false
        }
    }
}

fn latest_compatible_version(
    available: &[AvailableVersion],
    constraints: &[&VersionReq],
) -> Option<Version> {
    available
        .iter()
        .filter(|version| constraints.iter().all(|req| req.matches(&version.version)))
        .max_by(|a, b| a.version.cmp(&b.version))
        .map(|version| version.version.clone())
}
