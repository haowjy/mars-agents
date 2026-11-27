//! Upgrade hint calculation for sync reports.

use mars_schema::version::Constraint;

use crate::config::EffectiveConfig;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::LockFile;
use crate::resolve::{
    ResolveOptions, ResolvedGraph, ResolvedNode, SourceProvider, upgrade_candidate,
};
use crate::source::AvailableVersion;
use crate::types::{SourceId, SourceName};

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
) -> Result<Option<UpgradeCheck>, MarsError> {
    let (
        Some(fetch_url),
        SourceId::Git {
            subpath,
            plugin: None,
            ..
        },
    ) = (&node.fetch_url, &node.source_id)
    else {
        return Ok(None);
    };
    let name = &node.source_name;
    let available = provider.list_versions(fetch_url)?;
    let constraints: Vec<(&str, &Constraint)> = graph
        .version_constraints
        .get(name)
        .into_iter()
        .flatten()
        .map(|(requester, constraint)| (requester.as_str(), constraint))
        .collect();
    // A node with no version is a bare commit of a repository that had no tag when it was
    // locked: the first tag it gains is an upgrade.
    let candidate = upgrade_candidate(
        name,
        fetch_url,
        subpath.as_ref(),
        &available,
        node.resolved_ref.version.as_ref(),
        &constraints,
        provider,
        options,
    )?
    .cloned();
    Ok(Some(UpgradeCheck {
        available,
        candidate,
    }))
}

/// A dependency whose version differs from the one the lock had (a move) or from the one
/// `mars upgrade` would pick (an upgrade waiting). `from` and `to` are a tag, or a short commit
/// for a dependency with no tag.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VersionStep {
    pub name: SourceName,
    pub from: String,
    pub to: String,
    /// Required by another dependency and not named in `mars.toml`.
    pub transitive: bool,
}

impl VersionStep {
    pub fn describe(&self) -> String {
        let marker = if self.transitive { " (transitive)" } else { "" };
        format!("{} {} → {}{marker}", self.name, self.from, self.to)
    }
}

pub(crate) fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}

/// The git dependencies `mars upgrade` would move: the version it would pick is newer than the
/// one resolved, or an unversioned dependency's repository has gained a tag. A layer's
/// dependencies are not the project's to move (the user level's replay its own lock, which
/// `mars --user upgrade` moves), so they are not asked about and the sync contacts no remote
/// for them.
pub(crate) fn pending_upgrades(
    graph: &ResolvedGraph,
    effective: &EffectiveConfig,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Vec<VersionStep> {
    graph
        .nodes
        .values()
        .filter(|node| !crate::layer::is_machine_local(&node.source_name))
        .filter_map(|node| {
            let candidate = match check_upgrade(node, graph, provider, options) {
                Ok(checked) => checked?.candidate?,
                Err(err) => {
                    diag.warn(
                        "upgrade-hint-unavailable",
                        format!(
                            "could not check newer versions of `{}`: {err}",
                            node.source_name
                        ),
                    );
                    return None;
                }
            };
            Some(VersionStep {
                name: node.source_name.clone(),
                from: node.resolved_ref.version_tag.clone().unwrap_or_else(|| {
                    node.resolved_ref
                        .commit
                        .as_ref()
                        .map_or_else(|| "-".to_string(), |commit| short_commit(commit.as_ref()))
                }),
                to: candidate.tag,
                transitive: !effective.dependencies.contains_key(&node.source_name),
            })
        })
        .collect()
}

/// The dependencies this run resolved to a different commit than `old_lock` had. A move that
/// keeps its version label (a branch that advanced, a tag pointed at a new commit) is labelled
/// with both commits, so the two sides differ.
pub(crate) fn moved_dependencies(
    old_lock: &LockFile,
    graph: &ResolvedGraph,
    effective: &EffectiveConfig,
) -> Vec<VersionStep> {
    graph
        .nodes
        .values()
        .filter_map(|node| {
            let old = old_lock.dependencies.get(&node.source_name)?;
            let new_commit = node.resolved_ref.commit.as_ref().map(AsRef::as_ref)?;
            let old_commit = old.commit.as_deref();
            if old_commit == Some(new_commit) {
                return None;
            }
            let old_version = old.version.as_deref();
            let new_version = node.resolved_ref.version_tag.as_deref();
            let label = |version: Option<&str>, commit: Option<&str>| {
                let commit = commit.map_or_else(|| "-".to_string(), short_commit);
                match version {
                    Some(version) if old_version == new_version => format!("{version}@{commit}"),
                    Some(version) => version.to_string(),
                    None => commit,
                }
            };
            Some(VersionStep {
                name: node.source_name.clone(),
                from: label(old_version, old_commit),
                to: label(new_version, Some(new_commit)),
                transitive: !effective.dependencies.contains_key(&node.source_name),
            })
        })
        .collect()
}
