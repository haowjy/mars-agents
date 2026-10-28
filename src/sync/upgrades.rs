//! Upgrade hint calculation for sync reports.

use mars_schema::version::Constraint;

use crate::diagnostic::DiagnosticCollector;
use crate::resolve::{ResolveOptions, ResolvedGraph, SourceProvider, upgrade_candidate};
use crate::types::SourceId;

/// How many git dependencies `mars upgrade` would move: the version it would pick is newer than
/// the one resolved.
pub(crate) fn count_compatible_upgrades(
    graph: &ResolvedGraph,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> usize {
    graph
        .nodes
        .values()
        .filter(|node| {
            let Some(resolved) = node.resolved_ref.version.as_ref() else {
                return false;
            };
            let SourceId::Git { url, subpath, plugin } = &node.source_id else {
                return false;
            };
            // A marketplace plugin's manifest is not the repository's `mars.toml`.
            if plugin.is_some() {
                return false;
            }
            let Some(constraints) = graph.version_constraints.get(&node.source_name) else {
                return false;
            };
            let constraints: Vec<(&str, &Constraint)> = constraints
                .iter()
                .map(|(requester, constraint)| (requester.as_str(), constraint))
                .collect();
            let name = &node.source_name;
            match provider.list_versions(url) {
                Ok(available) => upgrade_candidate(
                    name,
                    url,
                    subpath.as_ref(),
                    &available,
                    &constraints,
                    provider,
                    options,
                    diag,
                )
                .is_some_and(|candidate| candidate.version > *resolved),
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
        })
        .count()
}
