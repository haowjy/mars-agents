use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use mars_schema::version::{
    Constraint, SelectError, SelectionPolicy, allows, keeps_lock, parse_tag, select, settle,
};
use semver::Version;

use crate::diagnostic::DiagnosticCollector;
use crate::error::{MarsError, ResolutionError};
use crate::lock::{LockFile, LockedSource};
use crate::source::{AvailableVersion, ResolvedRef};
use crate::types::{SourceId, SourceName, SourceUrl};

use super::EngineExclusions;
use super::SourceProvider;
use super::package::PendingSource;
use super::types::{ResolveOptions, ResolvedNode, VersionMode};

/// Resolve a single source to a concrete version/ref.
pub(crate) fn resolve_single_source(
    pending: &PendingSource,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    constraints: &HashMap<SourceName, Vec<(String, Constraint)>>,
    exclusions: &EngineExclusions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let mode = options.version_mode(&pending.name);
    match &pending.spec {
        crate::config::SourceSpec::Path(path) => {
            // Path sources: no version resolution, just use the path
            provider.fetch_path(path, pending.name.as_ref(), diag)
        }
        crate::config::SourceSpec::Git(git) => {
            // Lock is consulted for normal sync and frozen lock-exact mode; upgrade
            // targets intentionally ignore lock replay.
            let locked_source = match mode {
                VersionMode::Choose(SelectionPolicy::LatestOnly) => None,
                _ => locked_source_for_pending(pending, locked, mode, diag)?,
            };
            resolve_git_source(
                &pending.name,
                &git.url,
                constraints
                    .get(&pending.name)
                    .map(|c| c.as_slice())
                    .unwrap_or(&[]),
                provider,
                locked_source,
                mode,
                exclusions,
                diag,
            )
        }
    }
}

fn locked_source_for_pending<'a>(
    pending: &PendingSource,
    locked: Option<&'a LockFile>,
    mode: VersionMode,
    diag: &mut DiagnosticCollector,
) -> Result<Option<&'a LockedSource>, MarsError> {
    let Some(lock) = locked else {
        return Ok(None);
    };

    let Some(locked_source) = lock.dependencies.get(&pending.name) else {
        return Ok(None);
    };

    if locked_source_matches_pending(locked_source, pending) {
        return Ok(Some(locked_source));
    }

    let locked_identity = locked_source_identity_string(locked_source);
    let expected_identity = pending.source_id.to_string();
    if mode == VersionMode::Frozen {
        return Err(MarsError::FrozenViolation {
            message: format!(
                "--frozen lock entry for source `{}` does not match current source identity (lock: {locked_identity}, current: {expected_identity})",
                pending.name
            ),
        });
    }

    diag.warn(
        "stale-lock-source-identity",
        format!(
            "ignoring stale lock entry for `{}`: lock identity {locked_identity} does not match current identity {expected_identity}",
            pending.name
        ),
    );
    Ok(None)
}

pub(super) fn locked_source_matches_pending(
    locked_source: &LockedSource,
    pending: &PendingSource,
) -> bool {
    match &pending.source_id {
        SourceId::Git {
            url,
            subpath,
            plugin,
        } => {
            locked_source.path.is_none()
                && locked_source.plugin.as_ref() == plugin.as_ref()
                && locked_source
                    .url
                    .as_ref()
                    .is_some_and(|locked_url| git_urls_equivalent(locked_url, url))
                && locked_source.subpath.as_ref() == subpath.as_ref()
        }
        SourceId::Path {
            canonical,
            subpath,
            plugin,
        } => {
            locked_source.url.is_none()
                && locked_source.plugin.as_ref() == plugin.as_ref()
                && locked_source.subpath.as_ref() == subpath.as_ref()
                && locked_source.path.as_deref().is_some_and(|locked_path| {
                    crate::target::paths_equivalent(locked_path, &canonical.to_string_lossy())
                })
        }
    }
}

fn git_urls_equivalent(locked_url: &SourceUrl, pending_url: &SourceUrl) -> bool {
    mars_schema::git_url::canonicalize_git_url(locked_url.as_ref())
        == mars_schema::git_url::canonicalize_git_url(pending_url.as_ref())
}

fn locked_source_identity_string(locked_source: &LockedSource) -> String {
    let source = match (
        &locked_source.url,
        &locked_source.path,
        &locked_source.subpath,
    ) {
        (Some(url), None, Some(subpath)) => format!("git:{url}@{subpath}"),
        (Some(url), None, None) => format!("git:{url}"),
        (None, Some(path), Some(subpath)) => format!("path:{path}@{subpath}"),
        (None, Some(path), None) => format!("path:{path}"),
        (Some(url), Some(path), subpath) => {
            format!(
                "invalid-lock-entry(url={url}, path={path}, subpath={})",
                subpath
                    .as_ref()
                    .map(|value| value.as_str())
                    .unwrap_or("none")
            )
        }
        (None, None, subpath) => format!(
            "invalid-lock-entry(url=none, path=none, subpath={})",
            subpath
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or("none")
        ),
    };
    match &locked_source.plugin {
        Some(plugin) => format!("{source} (plugin {plugin})"),
        None => source,
    }
}

fn replay_locked_semver_commit(
    name: &SourceName,
    url: &SourceUrl,
    provider: &dyn SourceProvider,
    locked_commit: &str,
    locked_version: &Version,
    locked_version_raw: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let mut resolved = provider.fetch_git_commit(url, locked_commit, name.as_ref(), diag)?;
    resolved.version = Some(locked_version.clone());
    resolved.version_tag = Some(
        locked_version_raw
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("v{locked_version}")),
    );
    Ok(resolved)
}

fn annotate_refpin_resolution(mut resolved: ResolvedRef, ref_name: &str) -> ResolvedRef {
    // Ref-pinned resolutions are not semver selections, but we persist the
    // selector in `version_tag` so lock replay can verify selector equality.
    resolved.version = None;
    resolved.version_tag = Some(ref_name.to_string());
    resolved
}

fn resolve_ref_pin_source(
    name: &SourceName,
    url: &SourceUrl,
    ref_name: &str,
    provider: &dyn SourceProvider,
    locked_source: Option<&LockedSource>,
    mode: VersionMode,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let locked_commit = locked_source.and_then(|source| source.commit.as_deref());
    let locked_selector = locked_source.and_then(|source| source.version.as_deref());
    let selector_matches_lock = locked_selector.is_some_and(|selector| selector == ref_name);
    let preferred_commit = match mode {
        VersionMode::Choose(SelectionPolicy::LatestOnly) => None,
        VersionMode::Choose(SelectionPolicy::PreferLockThenLatest) => {
            if locked_commit.is_some() && !selector_matches_lock {
                let lock_selector_desc = locked_selector.unwrap_or("<missing>");
                diag.warn(
                    "locked-ref-selector-mismatch",
                    format!(
                        "ignoring locked commit for ref-pinned source `{name}` ({url}): lock selector `{lock_selector_desc}` does not match requested selector `{ref_name}`"
                    ),
                );
                None
            } else {
                locked_commit
            }
        }
        VersionMode::Frozen => {
            let source = locked_source.ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires lock entry for ref-pinned source `{name}` ({url})"
                ),
            })?;
            let selector = source.version.as_deref().ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires locked ref selector for ref-pinned source `{name}` ({url})"
                ),
            })?;
            if selector != ref_name {
                return Err(MarsError::FrozenViolation {
                    message: format!(
                        "--frozen lock selector `{selector}` for ref-pinned source `{name}` ({url}) does not match requested selector `{ref_name}`"
                    ),
                });
            }
            let commit = source
                .commit
                .as_deref()
                .ok_or_else(|| MarsError::FrozenViolation {
                    message: format!(
                        "--frozen requires locked commit for ref-pinned source `{name}` ({url})"
                    ),
                })?;
            Some(commit)
        }
    };

    if let Some(commit) = preferred_commit {
        return match provider.fetch_git_commit(url, commit, name.as_ref(), diag) {
            Ok(resolved_ref) => Ok(annotate_refpin_resolution(resolved_ref, ref_name)),
            Err(err @ MarsError::LockedCommitUnreachable { .. }) if mode == VersionMode::Frozen => {
                Err(err)
            }
            Err(MarsError::LockedCommitUnreachable {
                commit,
                url: source_url,
            }) => {
                diag.warn(
                    "locked-commit-unreachable",
                    format!(
                        "locked commit {commit} for {source_url} is unreachable; re-resolving ref `{ref_name}`"
                    ),
                );
                provider
                    .fetch_git_ref(url, ref_name, name.as_ref(), None, diag)
                    .map(|resolved_ref| annotate_refpin_resolution(resolved_ref, ref_name))
            }
            Err(err) => Err(err),
        };
    }

    provider
        .fetch_git_ref(url, ref_name, name.as_ref(), None, diag)
        .map(|resolved_ref| annotate_refpin_resolution(resolved_ref, ref_name))
}

fn resolve_untagged_source(
    name: &SourceName,
    url: &SourceUrl,
    provider: &dyn SourceProvider,
    locked_commit: Option<&str>,
    locked_commit_unreachable: bool,
    policy: SelectionPolicy,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    // No semver tags → treat as "latest commit", with locked-commit replay.
    let preferred_commit = match policy {
        SelectionPolicy::LatestOnly => None,
        SelectionPolicy::PreferLockThenLatest => {
            if locked_commit_unreachable {
                None
            } else {
                locked_commit
            }
        }
    };
    if let Some(commit) = preferred_commit {
        match provider.fetch_git_commit(url, commit, name.as_ref(), diag) {
            Ok(resolved) => return Ok(resolved),
            Err(MarsError::LockedCommitUnreachable {
                commit,
                url: source_url,
            }) => {
                diag.warn(
                    "locked-commit-unreachable",
                    format!(
                        "locked commit {commit} for {source_url} is unreachable; re-resolving from HEAD"
                    ),
                );
                return provider.fetch_git_ref(url, "HEAD", name.as_ref(), None, diag);
            }
            Err(err) => return Err(err),
        }
    }

    let resolved = provider.fetch_git_ref(url, "HEAD", name.as_ref(), None, diag)?;
    Ok(resolved)
}

/// Resolve a git source: list versions, intersect constraints, select version.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_git_source(
    name: &SourceName,
    url: &SourceUrl,
    constraints: &[(String, Constraint)],
    provider: &dyn SourceProvider,
    locked_source: Option<&LockedSource>,
    mode: VersionMode,
    exclusions: &EngineExclusions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let requests: Vec<(&str, &Constraint)> = constraints
        .iter()
        .map(|(requester, constraint)| (requester.as_str(), constraint))
        .collect();
    if let Some(reference) =
        settle(&requests).map_err(|error| ResolutionError::VersionConflict {
            name: name.to_string(),
            message: error.to_string(),
        })?
    {
        return resolve_ref_pin_source(name, url, reference, provider, locked_source, mode, diag);
    }

    let locked_commit = locked_source.and_then(|ls| ls.commit.as_deref());
    let locked_version_raw = locked_source.and_then(|ls| ls.version.as_deref());
    let locked_version = locked_version_raw.and_then(parse_tag);
    let excluded: HashSet<Version> = exclusions
        .keys()
        .filter(|(source, _)| source == name)
        .map(|(_, version)| version.clone())
        .collect();

    let policy = match mode {
        VersionMode::Choose(policy) => policy,
        VersionMode::Frozen => {
            let has_semver = requests
                .iter()
                .any(|(_, constraint)| matches!(constraint, Constraint::Semver(_)));
            if locked_version_raw.is_none() && !has_semver {
                let locked_commit = locked_commit.ok_or_else(|| MarsError::FrozenViolation {
                    message: format!(
                        "--frozen requires locked commit for unpinned source `{name}` ({url})"
                    ),
                })?;
                return provider.fetch_git_commit(url, locked_commit, name.as_ref(), diag);
            }
            if locked_source.is_none() {
                return Err(MarsError::FrozenViolation {
                    message: format!("--frozen requires lock entry for source `{name}` ({url})"),
                });
            }
            let locked_version = locked_version.ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires parseable locked semver version for source `{name}` ({url}); found {:?}",
                    locked_version_raw
                ),
            })?;
            let locked_commit = locked_commit.ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires locked commit for semver source `{name}` ({url})"
                ),
            })?;
            if !allows(&requests, &locked_version, &excluded) {
                let why = if excluded.contains(&locked_version) {
                    "its engine requirements"
                } else {
                    "current constraints"
                };
                return Err(MarsError::FrozenViolation {
                    message: format!(
                        "--frozen lock version {locked_version} for `{name}` is incompatible with {why}"
                    ),
                });
            }
            return replay_locked_semver_commit(
                name,
                url,
                provider,
                locked_commit,
                &locked_version,
                locked_version_raw,
                diag,
            );
        }
    };

    let mut locked_commit_unreachable = false;
    if let (Some(locked_version), Some(locked_commit)) = (locked_version.as_ref(), locked_commit)
        && keeps_lock(&requests, locked_version, policy, &excluded)
    {
        match replay_locked_semver_commit(
            name,
            url,
            provider,
            locked_commit,
            locked_version,
            locked_version_raw,
            diag,
        ) {
            Ok(resolved) => return Ok(resolved),
            Err(MarsError::LockedCommitUnreachable {
                commit,
                url: source_url,
            }) => {
                diag.warn(
                    "locked-commit-unreachable",
                    format!(
                        "locked commit {commit} for {source_url} is unreachable; re-resolving from current tags"
                    ),
                );
                locked_commit_unreachable = true;
            }
            Err(err) => return Err(err),
        }
    }

    // List available versions
    let available = provider.list_versions(url)?;
    let Some(selected) = select_version(
        name,
        &available,
        &requests,
        locked_version.as_ref(),
        policy,
        &excluded,
    )?
    else {
        return resolve_untagged_source(
            name,
            url,
            provider,
            locked_commit,
            locked_commit_unreachable,
            policy,
            diag,
        );
    };

    let should_try_locked_commit = !locked_commit_unreachable
        && locked_commit.is_some()
        && locked_version.as_ref().is_some_and(|version| {
            selected.version == *version && keeps_lock(&requests, version, policy, &excluded)
        });

    let preferred_commit = if should_try_locked_commit {
        locked_commit
    } else {
        None
    };

    match provider.fetch_git_version(url, selected, name.as_ref(), preferred_commit, diag) {
        Ok(resolved) => Ok(resolved),
        Err(MarsError::LockedCommitUnreachable {
            commit,
            url: source_url,
        }) => {
            diag.warn(
                "locked-commit-unreachable",
                format!(
                    "locked commit {commit} for {source_url} is unreachable; re-resolving from tag"
                ),
            );
            provider.fetch_git_version(url, selected, name.as_ref(), None, diag)
        }
        Err(err) => Err(err),
    }
}

/// Select a concrete version from available versions, respecting constraints and the sources'
/// engine exclusions; the rule is `mars_schema::version::select`, this maps its refusals to
/// the resolver's errors. `None` is a repository with no version tags.
fn select_version<'a>(
    source_name: &SourceName,
    available: &'a [AvailableVersion],
    constraints: &[(&str, &Constraint)],
    locked: Option<&Version>,
    policy: SelectionPolicy,
    excluded: &HashSet<Version>,
) -> Result<Option<&'a AvailableVersion>, MarsError> {
    let versions: Vec<Version> = available.iter().map(|av| av.version.clone()).collect();
    match select(&versions, constraints, locked, policy, excluded) {
        Ok(index) => Ok(index.map(|index| &available[index])),
        Err(error @ SelectError::NoSatisfying { .. }) => Err(ResolutionError::VersionConflict {
            name: source_name.to_string(),
            message: error.to_string(),
        }
        .into()),
    }
}

/// Warn once, on the settled graph, about a `Latest` request beside a pin: it holds, but asked
/// for the newest and got a fixed ref. (Not per resolution pass, which a restart repeats.)
pub(crate) fn warn_version_drift(
    nodes: &IndexMap<SourceName, ResolvedNode>,
    constraints: &HashMap<SourceName, Vec<(String, Constraint)>>,
    diag: &mut DiagnosticCollector,
) {
    for (name, constraint_list) in constraints {
        let Some(node) = nodes.get(name) else {
            continue;
        };
        let resolved = &node.resolved_ref;
        let Some(pin) = resolved
            .version
            .is_none()
            .then_some(resolved.version_tag.as_deref())
            .flatten()
        else {
            continue;
        };
        for (requester, constraint) in constraint_list {
            if matches!(constraint, Constraint::Latest) {
                diag.warn(
                    "potential-version-drift",
                    format!(
                        "potential version drift: package '{name}' requested as latest by {requester} but pinned to ref:{pin}"
                    ),
                );
            }
        }
    }
}
