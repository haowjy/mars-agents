//! Upgrade hint calculation for sync reports.

use mars_schema::version::Constraint;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::resolve::{
    ResolveOptions, ResolvedGraph, ResolvedNode, SourceProvider, upgrade_candidate,
};
use crate::source::AvailableVersion;
use crate::types::SourceId;

/// What [`check_upgrade`] found: every tag the repository offers, and the one to move to.
pub(crate) struct UpgradeCheck {
    pub available: Vec<AvailableVersion>,
    pub candidate: Option<AvailableVersion>,
}

/// The versions a git dependency offers and the one `mars upgrade` would move it to, if any.
/// `None` for a node with no version to move: a path dependency or a marketplace plugin (whose
/// manifest is not the repository's `mars.toml`).
pub(crate) fn check_upgrade(
    node: &ResolvedNode,
    graph: &ResolvedGraph,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<Option<UpgradeCheck>, MarsError> {
    let SourceId::Git {
        url,
        subpath,
        plugin: None,
    } = &node.source_id
    else {
        return Ok(None);
    };
    let name = &node.source_name;
    let available = provider.list_versions(url)?;
    let constraints: Vec<(&str, &Constraint)> = graph
        .version_constraints
        .get(name)
        .into_iter()
        .flatten()
        .map(|(requester, constraint)| (requester.as_str(), constraint))
        .collect();
    let candidate = match node.resolved_ref.version.as_ref() {
        Some(resolved) => upgrade_candidate(
            name,
            url,
            subpath.as_ref(),
            &available,
            Some(resolved),
            &constraints,
            provider,
            options,
            diag,
        )?
        .cloned(),
        None => None,
    };
    Ok(Some(UpgradeCheck {
        available,
        candidate,
    }))
}

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
        .filter(
            |node| match check_upgrade(node, graph, provider, options, diag) {
                Ok(checked) => checked.is_some_and(|checked| checked.candidate.is_some()),
                Err(err) => {
                    diag.warn(
                        "upgrade-hint-unavailable",
                        format!(
                            "could not check newer versions of `{}`: {err}",
                            node.source_name
                        ),
                    );
                    false
                }
            },
        )
        .count()
}
