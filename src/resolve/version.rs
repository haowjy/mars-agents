use std::collections::HashMap;

use indexmap::IndexMap;
use semver::{Version, VersionReq};

use crate::diagnostic::DiagnosticCollector;
use crate::error::{MarsError, ResolutionError};
use crate::lock::{LockFile, LockedSource};
use crate::source::{AvailableVersion, ResolvedRef};
use crate::types::{SourceId, SourceName, SourceUrl};

use super::SourceProvider;
use super::package::PendingSource;
use super::types::{
    ResolveMode, ResolveOptions, ResolvedNode, VersionConstraint, VersionSelectionPolicy,
};

/// Resolve a single source to a concrete version/ref.
pub(crate) fn resolve_single_source(
    pending: &PendingSource,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    constraints: &HashMap<SourceName, Vec<(String, VersionConstraint)>>,
    diag: &mut DiagnosticCollector,
) -> Result<(ResolvedRef, Option<Version>), MarsError> {
    let selection_policy = options.version_selection_policy(&pending.name);
    match &pending.spec {
        crate::config::SourceSpec::Path(path) => {
            // Path sources: no version resolution, just use the path
            provider
                .fetch_path(path, pending.name.as_ref(), diag)
                .map(|resolved_ref| (resolved_ref, None))
        }
        crate::config::SourceSpec::Git(git) => {
            // Lock is consulted for normal sync and frozen lock-exact mode; upgrade
            // targets intentionally ignore lock replay.
            let locked_source = match selection_policy {
                VersionSelectionPolicy::LatestOnly => None,
                VersionSelectionPolicy::PreferLockThenLatest | VersionSelectionPolicy::LockOnly => {
                    locked_source_for_pending(pending, locked, selection_policy, diag)?
                }
            };
            let fetch_upgrade_metadata = matches!(options.mode, ResolveMode::Upgrade { .. });
            resolve_git_source(
                &pending.name,
                &git.url,
                constraints
                    .get(&pending.name)
                    .map(|c| c.as_slice())
                    .unwrap_or(&[]),
                provider,
                locked_source,
                selection_policy,
                fetch_upgrade_metadata,
                diag,
            )
        }
    }
}

fn locked_source_for_pending<'a>(
    pending: &PendingSource,
    locked: Option<&'a LockFile>,
    selection_policy: VersionSelectionPolicy,
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
    if selection_policy == VersionSelectionPolicy::LockOnly {
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

fn locked_source_matches_pending(locked_source: &LockedSource, pending: &PendingSource) -> bool {
    match &pending.source_id {
        SourceId::Git { url, subpath } => {
            locked_source.path.is_none()
                && locked_source
                    .url
                    .as_ref()
                    .is_some_and(|locked_url| git_urls_equivalent(locked_url, url))
                && locked_source.subpath.as_ref() == subpath.as_ref()
        }
        SourceId::Path { canonical, subpath } => {
            locked_source.url.is_none()
                && locked_source.subpath.as_ref() == subpath.as_ref()
                && locked_source.path.as_deref().is_some_and(|locked_path| {
                    crate::target::paths_equivalent(locked_path, &canonical.to_string_lossy())
                })
        }
    }
}

fn git_urls_equivalent(locked_url: &SourceUrl, pending_url: &SourceUrl) -> bool {
    crate::source::canonical::canonicalize_git_url(locked_url.as_ref())
        == crate::source::canonical::canonicalize_git_url(pending_url.as_ref())
}

fn locked_source_identity_string(locked_source: &LockedSource) -> String {
    match (
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
    }
}

fn semver_constraints_satisfied(version: &Version, constraints: &[(&str, &VersionReq)]) -> bool {
    constraints.iter().all(|(_, req)| req.matches(version))
}

fn latest_version_metadata(
    name: &SourceName,
    url: &SourceUrl,
    provider: &dyn SourceProvider,
    diag: &mut DiagnosticCollector,
) -> Option<Version> {
    match provider.list_versions(url) {
        Ok(available) => available
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .map(|v| v.version.clone()),
        Err(err) => {
            diag.warn(
                "latest-version-unavailable",
                format!(
                    "resolved `{name}` from lock replay but could not list current versions for upgrade metadata ({url}): {err}"
                ),
            );
            None
        }
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
    selection_policy: VersionSelectionPolicy,
    diag: &mut DiagnosticCollector,
) -> Result<(ResolvedRef, Option<Version>), MarsError> {
    let locked_commit = locked_source.and_then(|source| source.commit.as_deref());
    let locked_selector = locked_source.and_then(|source| source.version.as_deref());
    let selector_matches_lock = locked_selector.is_some_and(|selector| selector == ref_name);
    let preferred_commit = match selection_policy {
        VersionSelectionPolicy::LatestOnly => None,
        VersionSelectionPolicy::PreferLockThenLatest => {
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
        VersionSelectionPolicy::LockOnly => {
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
            Ok(resolved_ref) => Ok((annotate_refpin_resolution(resolved_ref, ref_name), None)),
            Err(err @ MarsError::LockedCommitUnreachable { .. })
                if selection_policy == VersionSelectionPolicy::LockOnly =>
            {
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
                    .map(|resolved_ref| (annotate_refpin_resolution(resolved_ref, ref_name), None))
            }
            Err(err) => Err(err),
        };
    }

    provider
        .fetch_git_ref(url, ref_name, name.as_ref(), None, diag)
        .map(|resolved_ref| (annotate_refpin_resolution(resolved_ref, ref_name), None))
}

fn resolve_untagged_source(
    name: &SourceName,
    url: &SourceUrl,
    provider: &dyn SourceProvider,
    locked_commit: Option<&str>,
    locked_commit_unreachable: bool,
    selection_policy: VersionSelectionPolicy,
    diag: &mut DiagnosticCollector,
) -> Result<(ResolvedRef, Option<Version>), MarsError> {
    // No semver tags → treat as "latest commit", with locked-commit replay.
    let preferred_commit = match selection_policy {
        VersionSelectionPolicy::LatestOnly => None,
        VersionSelectionPolicy::PreferLockThenLatest => {
            if locked_commit_unreachable {
                None
            } else {
                locked_commit
            }
        }
        VersionSelectionPolicy::LockOnly => {
            let commit = locked_commit.ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires locked commit for untagged source `{name}` ({url})"
                ),
            })?;
            Some(commit)
        }
    };
    if let Some(commit) = preferred_commit {
        match provider.fetch_git_commit(url, commit, name.as_ref(), diag) {
            Ok(resolved) => return Ok((resolved, None)),
            Err(err @ MarsError::LockedCommitUnreachable { .. })
                if selection_policy == VersionSelectionPolicy::LockOnly =>
            {
                return Err(err);
            }
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
                return provider
                    .fetch_git_ref(url, "HEAD", name.as_ref(), None, diag)
                    .map(|resolved_ref| (resolved_ref, None));
            }
            Err(err) => return Err(err),
        }
    }

    let resolved = provider.fetch_git_ref(url, "HEAD", name.as_ref(), None, diag)?;
    Ok((resolved, None))
}

/// Resolve a git source: list versions, intersect constraints, select version.
pub(crate) fn resolve_git_source(
    name: &SourceName,
    url: &SourceUrl,
    constraints: &[(String, VersionConstraint)],
    provider: &dyn SourceProvider,
    locked_source: Option<&LockedSource>,
    selection_policy: VersionSelectionPolicy,
    fetch_upgrade_metadata: bool,
    diag: &mut DiagnosticCollector,
) -> Result<(ResolvedRef, Option<Version>), MarsError> {
    let has_latest_constraint = constraints
        .iter()
        .any(|(_, constraint)| matches!(constraint, VersionConstraint::Latest));

    // If any constraint is a ref pin, use the first pin encountered
    // (multiple ref pins for the same source is likely an error, but we'll use first).
    if let Some(ref_name) = constraints
        .iter()
        .find_map(|(_, constraint)| match constraint {
            VersionConstraint::RefPin(ref_name) => Some(ref_name.as_str()),
            _ => None,
        })
    {
        return resolve_ref_pin_source(
            name,
            url,
            ref_name,
            provider,
            locked_source,
            selection_policy,
            diag,
        );
    }

    let locked_commit = locked_source.and_then(|ls| ls.commit.as_deref());

    // Collect all semver constraints
    let semver_reqs: Vec<(&str, &VersionReq)> = constraints
        .iter()
        .filter_map(|(requester, c)| match c {
            VersionConstraint::Semver(req) => Some((requester.as_str(), req)),
            _ => None,
        })
        .collect();

    // Get locked version for this source (if any)
    let locked_version_raw = locked_source.and_then(|ls| ls.version.as_deref());
    let locked_version = locked_source
        .and_then(|ls| ls.version.as_ref())
        .and_then(|v| {
            let v = v.strip_prefix('v').unwrap_or(v);
            Version::parse(v).ok()
        });

    if selection_policy == VersionSelectionPolicy::LockOnly
        && (locked_version_raw.is_some() || !semver_reqs.is_empty())
    {
        let source = locked_source.ok_or_else(|| MarsError::FrozenViolation {
            message: format!("--frozen requires lock entry for source `{name}` ({url})"),
        })?;
        let locked_version = locked_version.ok_or_else(|| MarsError::FrozenViolation {
            message: format!(
                "--frozen requires parseable locked semver version for source `{name}` ({url}); found {:?}",
                locked_version_raw
            ),
        })?;
        let locked_commit = source
            .commit
            .as_deref()
            .ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires locked commit for semver source `{name}` ({url})"
                ),
            })?;
        if !semver_constraints_satisfied(&locked_version, &semver_reqs) {
            return Err(MarsError::FrozenViolation {
                message: format!(
                    "--frozen lock version {locked_version} for `{name}` is incompatible with current constraints"
                ),
            });
        }
        let resolved = replay_locked_semver_commit(
            name,
            url,
            provider,
            locked_commit,
            &locked_version,
            locked_version_raw,
            diag,
        )?;
        return Ok((resolved, None));
    }

    if selection_policy == VersionSelectionPolicy::LockOnly
        && locked_version_raw.is_none()
        && semver_reqs.is_empty()
    {
        let locked_commit = locked_source
            .and_then(|source| source.commit.as_deref())
            .ok_or_else(|| MarsError::FrozenViolation {
                message: format!(
                    "--frozen requires locked commit for unpinned source `{name}` ({url})"
                ),
            })?;
        let resolved = provider.fetch_git_commit(url, locked_commit, name.as_ref(), diag)?;
        return Ok((resolved, None));
    }

    let mut locked_commit_unreachable = false;
    if selection_policy == VersionSelectionPolicy::PreferLockThenLatest
        && !has_latest_constraint
        && let (Some(locked_version), Some(locked_commit)) =
            (locked_version.as_ref(), locked_commit)
        && semver_constraints_satisfied(locked_version, &semver_reqs)
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
            Ok(resolved) => {
                let latest = if fetch_upgrade_metadata {
                    latest_version_metadata(name, url, provider, diag)
                } else {
                    None
                };
                return Ok((resolved, latest));
            }
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
    let latest = available
        .iter()
        .max_by(|a, b| a.version.cmp(&b.version))
        .map(|v| v.version.clone());

    if available.is_empty() {
        return resolve_untagged_source(
            name,
            url,
            provider,
            locked_commit,
            locked_commit_unreachable,
            selection_policy,
            diag,
        );
    }

    // Select version
    let select_policy = if selection_policy == VersionSelectionPolicy::PreferLockThenLatest
        && has_latest_constraint
    {
        VersionSelectionPolicy::LatestOnly
    } else {
        selection_policy
    };
    let selected = select_version(
        name,
        &available,
        &semver_reqs,
        locked_version.as_ref(),
        select_policy,
    )?;

    let should_try_locked_commit = !locked_commit_unreachable
        && select_policy != VersionSelectionPolicy::LatestOnly
        && locked_commit.is_some()
        && locked_version
            .as_ref()
            .is_some_and(|version| selected.version == *version);

    let preferred_commit = if should_try_locked_commit {
        locked_commit
    } else {
        None
    };

    match provider.fetch_git_version(url, selected, name.as_ref(), preferred_commit, diag) {
        Ok(resolved) => Ok((resolved, latest)),
        Err(err @ MarsError::LockedCommitUnreachable { .. })
            if selection_policy == VersionSelectionPolicy::LockOnly =>
        {
            Err(err)
        }
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
            provider
                .fetch_git_version(url, selected, name.as_ref(), None, diag)
                .map(|resolved_ref| (resolved_ref, latest))
        }
        Err(err) => Err(err),
    }
}

/// Select a concrete version from available versions, respecting constraints.
///
/// - PreferLockThenLatest: use compatible locked version, fallback to newest compatible.
/// - LatestOnly: pick newest compatible and ignore lock.
/// - LockOnly: require compatible locked version and error otherwise.
pub(crate) fn select_version<'a>(
    source_name: &SourceName,
    available: &'a [AvailableVersion],
    constraints: &[(&str, &VersionReq)],
    locked: Option<&Version>,
    selection_policy: VersionSelectionPolicy,
) -> Result<&'a AvailableVersion, MarsError> {
    // Find all versions satisfying all constraints
    let satisfying: Vec<&AvailableVersion> = available
        .iter()
        .filter(|av| {
            if constraints.is_empty() {
                return true;
            }
            constraints.iter().all(|(_, req)| req.matches(&av.version))
        })
        .collect();

    if satisfying.is_empty() {
        // Build helpful error message listing all constraints
        let constraint_desc: Vec<String> = constraints
            .iter()
            .map(|(requester, req)| format!("  `{requester}` requires {req}"))
            .collect();

        let available_desc: Vec<String> =
            available.iter().map(|av| av.version.to_string()).collect();

        return Err(ResolutionError::VersionConflict {
            name: source_name.to_string(),
            message: format!(
                "no version satisfies all constraints:\n{}\navailable versions: [{}]",
                constraint_desc.join("\n"),
                available_desc.join(", ")
            ),
        }
        .into());
    }

    let locked_satisfying = locked.and_then(|locked_ver| {
        satisfying
            .iter()
            .find(|av| av.version == *locked_ver)
            .copied()
    });
    let newest_satisfying = satisfying.last().copied().expect("satisfying is non-empty");

    match selection_policy {
        VersionSelectionPolicy::PreferLockThenLatest => {
            Ok(locked_satisfying.unwrap_or(newest_satisfying))
        }
        VersionSelectionPolicy::LatestOnly => Ok(newest_satisfying),
        VersionSelectionPolicy::LockOnly => {
            if let Some(locked_selected) = locked_satisfying {
                Ok(locked_selected)
            } else if let Some(locked_ver) = locked {
                Err(MarsError::FrozenViolation {
                    message: format!(
                        "--frozen lock version {locked_ver} for `{source_name}` is incompatible with current constraints or unavailable"
                    ),
                })
            } else {
                Err(MarsError::FrozenViolation {
                    message: format!(
                        "--frozen requires lock version for `{source_name}` but no lock version was found"
                    ),
                })
            }
        }
    }
}

/// Validate that all constraints are satisfied by the resolved versions.
///
/// This catches cases where a source was resolved before all constraints
/// were known (e.g., a later transitive dep adds a new constraint on an
/// already-resolved source).
pub(crate) fn validate_all_constraints(
    nodes: &IndexMap<SourceName, ResolvedNode>,
    constraints: &HashMap<SourceName, Vec<(String, VersionConstraint)>>,
) -> Result<(), MarsError> {
    for (name, constraint_list) in constraints {
        let node = match nodes.get(name) {
            Some(n) => n,
            None => continue, // Should not happen, but be safe
        };

        // Only validate semver constraints against resolved versions
        if let Some(ref resolved_ver) = node.resolved_ref.version {
            for (requester, constraint) in constraint_list {
                if let VersionConstraint::Semver(req) = constraint
                    && !req.matches(resolved_ver)
                {
                    return Err(ResolutionError::VersionConflict {
                        name: name.to_string(),
                        message: format!(
                            "resolved version {resolved_ver} does not satisfy \
                             constraint {req} (required by `{requester}`)"
                        ),
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}
