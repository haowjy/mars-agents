//! Which version of one source: the lock replay rules and the ladder of attempts (pin, lock,
//! tags, default branch), each an ordered fallback that warns instead of failing, except under
//! `--frozen`, where every fallback is an error.

use std::collections::HashSet;

use semver::Version;

use super::package::{Flow, Pending, Run, Stop};
use super::{
    Dependency, Failure, Level, LockedDependency, Mode, Need, Origin, Ref, Requester, SourceId,
    Spec,
};
use crate::git_url::canonicalize_git_url;
use crate::version::{
    Constraint, SelectError, SelectionPolicy, allows, keeps_lock, parse_tag, select, settle,
};

/// How a source's version is decided: the choice among tags, or `--frozen`, which replays the
/// lock exactly and is the resolver's own rule, not a way of choosing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VersionMode {
    Choose(SelectionPolicy),
    Frozen,
}

impl Mode {
    pub(super) fn version_mode(&self, name: &str) -> VersionMode {
        match self {
            Self::Frozen => VersionMode::Frozen,
            Self::Upgrade { targets } if targets.is_empty() || targets.contains(name) => {
                VersionMode::Choose(SelectionPolicy::LatestOnly)
            }
            Self::Sync | Self::Upgrade { .. } => {
                VersionMode::Choose(SelectionPolicy::PreferLockThenLatest)
            }
        }
    }
}

/// A selection can fail as a conflict, which the caller may report as the engine exclusions that
/// caused it.
pub(super) enum SingleError {
    Conflict(Box<Failure>),
    Stop(Stop),
}

impl From<Stop> for SingleError {
    fn from(stop: Stop) -> Self {
        Self::Stop(stop)
    }
}

impl SingleError {
    pub(super) fn stop(self, run: &mut Run<'_>) -> Stop {
        match self {
            Self::Stop(stop) => stop,
            Self::Conflict(failure) => match run.fail::<()>(*failure) {
                Err(stop) => stop,
                Ok(()) => unreachable!("a failure always leaves the branch"),
            },
        }
    }
}

type Single<T> = Result<T, SingleError>;

/// The path a package's files are read at: a plugin reads the whole repository (the marketplace's
/// root manifest, sentinel and local plugin folders), anything else its own folder.
fn read_path(dep: &Dependency) -> Option<&str> {
    if dep.plugin.is_some() {
        None
    } else {
        dep.subpath.as_ref().map(|subpath| subpath.as_str())
    }
}

impl<'a> Run<'a> {
    pub(super) fn resolve_single_source(&mut self, pending: &Pending<'_>) -> Single<Ref> {
        let dep = pending.dep;
        let mode = self.req.mode.version_mode(&dep.name);
        match &dep.spec {
            Spec::Path(path) => {
                let (canonical, _) = self.path_source(dep, path, None, dep.declared_in.as_ref())?;
                Ok(Ref {
                    version: None,
                    tag: None,
                    commit: None,
                    origin: Origin::Path { dir: canonical },
                })
            }
            Spec::Git { url, .. } => {
                // Upgrade targets intentionally ignore lock replay.
                let locked = match mode {
                    VersionMode::Choose(SelectionPolicy::LatestOnly) => None,
                    _ => self.locked_for(pending, mode)?,
                };
                self.resolve_git_source(pending, url, locked, mode)
            }
        }
    }

    pub(super) fn locked_for(
        &mut self,
        pending: &Pending<'_>,
        mode: VersionMode,
    ) -> Single<Option<&'a LockedDependency>> {
        let req = self.req;
        let Some(locked) = req.locked.get(&pending.dep.name) else {
            return Ok(None);
        };
        if locked_matches(locked, pending.id) {
            return Ok(Some(locked));
        }
        let locked_identity = identity_string(locked);
        let expected = pending.id.to_string();
        if mode == VersionMode::Frozen {
            return Err(self
                .fail::<()>(Failure::Frozen {
                    message: format!(
                        "--frozen lock entry for source `{}` does not match current source identity (lock: {locked_identity}, current: {expected})",
                        pending.dep.name
                    ),
                })
                .unwrap_err()
                .into());
        }
        self.note(
            Level::Warning,
            "stale-lock-source-identity",
            format!(
                "ignoring stale lock entry for `{}`: lock identity {locked_identity} does not match current identity {expected}",
                pending.dep.name
            ),
            false,
        );
        Ok(None)
    }

    fn frozen<T>(&mut self, message: String) -> Single<T> {
        Err(self
            .fail::<T>(Failure::Frozen { message })
            .err()
            .expect("fail never returns a value")
            .into())
    }

    /// The commit's files, or `None` when the repository does not have it.
    fn fetch_commit(&mut self, dep: &Dependency, url: &str, commit: &str) -> Flow<Option<()>> {
        Ok(self.tree(url, commit, read_path(dep))?.map(|_| ()))
    }

    fn unreachable<T>(&mut self, dep: &Dependency, url: &str, commit: &str) -> Flow<T> {
        let package = self.current_package();
        self.fail(Failure::Unavailable {
            package,
            need: Need::Tree {
                url: url.to_string(),
                commit: commit.to_string(),
                path: read_path(dep).map(str::to_string),
            },
            reason: "the commit does not exist".to_string(),
        })
    }

    /// A commit known to exist, fetched as `commit` of `url`.
    fn at_commit(&mut self, dep: &Dependency, url: &str, commit: &str) -> Flow<Ref> {
        if self.fetch_commit(dep, url, commit)?.is_none() {
            return self.unreachable(dep, url, commit);
        }
        Ok(git_ref(url, commit))
    }

    fn fetch_ref(&mut self, dep: &Dependency, url: &str, name: &str) -> Flow<Ref> {
        let commit = self.reference(url, name)?;
        self.at_commit(dep, url, &commit)
    }

    /// The lock row names a commit the remote lacks: say so, and resolve from the tags.
    fn locked_unreachable(&mut self, url: &str, commit: &str) {
        self.note(
            Level::Warning,
            "locked-commit-unreachable",
            format!(
                "locked commit {commit} for {url} is unreachable; re-resolving from current tags"
            ),
            false,
        );
    }

    fn replay(
        &mut self,
        dep: &Dependency,
        url: &str,
        commit: &str,
        version: &Version,
        raw: Option<&str>,
    ) -> Flow<Option<Ref>> {
        if self.fetch_commit(dep, url, commit)?.is_none() {
            return Ok(None);
        }
        Ok(Some(Ref {
            version: Some(version.clone()),
            tag: Some(
                raw.map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("v{version}")),
            ),
            ..git_ref(url, commit)
        }))
    }

    fn resolve_ref_pin(
        &mut self,
        pending: &Pending<'_>,
        url: &str,
        ref_name: &str,
        locked: Option<&LockedDependency>,
        mode: VersionMode,
    ) -> Single<Ref> {
        let dep = pending.dep;
        let name = &dep.name;
        let locked_commit = locked.and_then(|source| source.commit.as_deref());
        let locked_selector = locked.and_then(|source| source.version.as_deref());
        let selector_matches_lock = locked_selector.is_some_and(|selector| selector == ref_name);
        let preferred = match mode {
            VersionMode::Choose(SelectionPolicy::LatestOnly) => None,
            VersionMode::Choose(SelectionPolicy::PreferLockThenLatest) => {
                if locked_commit.is_some() && !selector_matches_lock {
                    let selector = locked_selector.unwrap_or("<missing>");
                    self.note(
                        Level::Warning,
                        "locked-ref-selector-mismatch",
                        format!(
                            "ignoring locked commit for ref-pinned source `{name}` ({url}): lock selector `{selector}` does not match requested selector `{ref_name}`"
                        ),
                        false,
                    );
                    None
                } else {
                    locked_commit
                }
            }
            VersionMode::Frozen => {
                let Some(source) = locked else {
                    return self.frozen(format!(
                        "--frozen requires lock entry for ref-pinned source `{name}` ({url})"
                    ));
                };
                let Some(selector) = source.version.as_deref() else {
                    return self.frozen(format!(
                        "--frozen requires locked ref selector for ref-pinned source `{name}` ({url})"
                    ));
                };
                if selector != ref_name {
                    return self.frozen(format!(
                        "--frozen lock selector `{selector}` for ref-pinned source `{name}` ({url}) does not match requested selector `{ref_name}`"
                    ));
                }
                let Some(commit) = source.commit.as_deref() else {
                    return self.frozen(format!(
                        "--frozen requires locked commit for ref-pinned source `{name}` ({url})"
                    ));
                };
                Some(commit)
            }
        };

        // Ref-pinned resolutions are not semver selections, but the selector is kept as the tag
        // so lock replay can verify selector equality.
        let pinned = |resolved: Ref| Ref {
            version: None,
            tag: Some(ref_name.to_string()),
            ..resolved
        };
        if let Some(commit) = preferred {
            if self.fetch_commit(dep, url, commit)?.is_some() {
                return Ok(pinned(git_ref(url, commit)));
            }
            if mode == VersionMode::Frozen {
                return Err(self.unreachable::<()>(dep, url, commit).unwrap_err().into());
            }
            self.note(
                Level::Warning,
                "locked-commit-unreachable",
                format!(
                    "locked commit {commit} for {url} is unreachable; re-resolving ref `{ref_name}`"
                ),
                false,
            );
        }
        Ok(pinned(self.fetch_ref(dep, url, ref_name)?))
    }

    fn resolve_untagged(
        &mut self,
        dep: &Dependency,
        url: &str,
        locked_commit: Option<&str>,
        locked_unreachable: bool,
        policy: SelectionPolicy,
    ) -> Single<Ref> {
        let preferred = match policy {
            SelectionPolicy::LatestOnly => None,
            SelectionPolicy::PreferLockThenLatest if locked_unreachable => None,
            SelectionPolicy::PreferLockThenLatest => locked_commit,
        };
        if let Some(commit) = preferred {
            if self.fetch_commit(dep, url, commit)?.is_some() {
                return Ok(git_ref(url, commit));
            }
            self.note(
                Level::Warning,
                "locked-commit-unreachable",
                format!("locked commit {commit} for {url} is unreachable; re-resolving from HEAD"),
                false,
            );
        }
        Ok(self.fetch_ref(dep, url, "HEAD")?)
    }

    fn resolve_git_source(
        &mut self,
        pending: &Pending<'_>,
        url: &str,
        locked: Option<&LockedDependency>,
        mode: VersionMode,
    ) -> Single<Ref> {
        let dep = pending.dep;
        let name = &dep.name;
        let constraints = self.constraints_of(name);
        let requests: Vec<(&str, &Constraint)> = constraints
            .iter()
            .map(|(requester, constraint)| (requester.as_str(), constraint))
            .collect();
        let conflict = |detail: String| {
            SingleError::Conflict(Box::new(Failure::Conflict {
                name: name.clone(),
                requesters: constraints
                    .iter()
                    .map(|(by, constraint)| Requester {
                        by: by.clone(),
                        constraint: constraint.to_string(),
                    })
                    .collect(),
                detail,
            }))
        };
        if let Some(reference) = settle(&requests).map_err(|error| conflict(error.to_string()))? {
            return self.resolve_ref_pin(pending, url, reference, locked, mode);
        }

        let locked_commit = locked.and_then(|source| source.commit.as_deref());
        let locked_version_raw = locked.and_then(|source| source.version.as_deref());
        let locked_version = locked_version_raw.and_then(parse_tag);
        let excluded: HashSet<Version> = self
            .exclusions
            .keys()
            .filter(|(source, _)| source == pending.id)
            .map(|(_, version)| version.clone())
            .collect();

        let policy = match mode {
            VersionMode::Choose(policy) => policy,
            VersionMode::Frozen => {
                let has_semver = requests
                    .iter()
                    .any(|(_, constraint)| matches!(constraint, Constraint::Semver(_)));
                // A lock row with a selector that is no version was written under a ref request
                // the walk has not met yet (a package that asks for itself at a ref, later): the
                // commit is replayed, and that request checks the selector when it comes.
                if locked_version.is_none() && !has_semver {
                    let Some(commit) = locked_commit else {
                        return self.frozen(format!(
                            "--frozen requires locked commit for unpinned source `{name}` ({url})"
                        ));
                    };
                    return Ok(self.at_commit(dep, url, commit)?);
                }
                if locked.is_none() {
                    return self.frozen(format!(
                        "--frozen requires lock entry for source `{name}` ({url})"
                    ));
                }
                let Some(locked_version) = locked_version else {
                    return self.frozen(format!(
                        "--frozen requires parseable locked semver version for source `{name}` ({url}); found {locked_version_raw:?}"
                    ));
                };
                let Some(commit) = locked_commit else {
                    return self.frozen(format!(
                        "--frozen requires locked commit for semver source `{name}` ({url})"
                    ));
                };
                if !allows(&requests, &locked_version, &excluded) {
                    let why = if excluded.contains(&locked_version) {
                        "its engine requirements"
                    } else {
                        "current constraints"
                    };
                    return self.frozen(format!(
                        "--frozen lock version {locked_version} for `{name}` is incompatible with {why}"
                    ));
                }
                return match self.replay(dep, url, commit, &locked_version, locked_version_raw)? {
                    Some(resolved) => Ok(resolved),
                    None => Err(self.unreachable::<()>(dep, url, commit).unwrap_err().into()),
                };
            }
        };

        // A lock row with a commit and no version is an untagged repository's HEAD; the same
        // rule keeps it. A selector that is no version (a ref pin since dropped) is not it.
        let mut locked_unreachable = false;
        if let Some(commit) = locked_commit
            && (locked_version.is_some() || locked_version_raw.is_none())
            && keeps_lock(&requests, locked_version.as_ref(), policy, &excluded)
        {
            let kept = match locked_version.as_ref() {
                Some(version) => self.replay(dep, url, commit, version, locked_version_raw)?,
                None => self
                    .fetch_commit(dep, url, commit)?
                    .map(|_| git_ref(url, commit)),
            };
            match kept {
                Some(resolved) => return Ok(resolved),
                None => {
                    self.locked_unreachable(url, commit);
                    locked_unreachable = true;
                }
            }
        }

        let available = self.tags(url)?;
        let versions: Vec<Version> = available.iter().map(|tag| tag.version.clone()).collect();
        let selected = match select(
            &versions,
            &requests,
            locked_version.as_ref(),
            policy,
            &excluded,
        ) {
            Ok(index) => index.map(|index| &available[index]),
            Err(error @ SelectError::NoSatisfying { .. }) => {
                return Err(conflict(error.to_string()));
            }
        };
        let Some(selected) = selected else {
            return self.resolve_untagged(dep, url, locked_commit, locked_unreachable, policy);
        };
        let resolved = Ref {
            version: Some(selected.version.clone()),
            tag: Some(selected.tag.clone()),
            ..git_ref(url, &selected.commit)
        };
        let commit = selected.commit.clone();
        self.at_commit(dep, url, &commit)?;
        Ok(resolved)
    }
}

fn git_ref(url: &str, commit: &str) -> Ref {
    Ref {
        version: None,
        tag: None,
        commit: Some(commit.to_string()),
        origin: Origin::Git {
            url: url.to_string(),
            commit: commit.to_string(),
        },
    }
}

/// Whether the lock row describes this source: the same repository (or path), subpath and
/// plugin.
pub(super) fn locked_matches(locked: &LockedDependency, id: &SourceId) -> bool {
    match id {
        SourceId::Git {
            url,
            subpath,
            plugin,
        } => {
            locked.path.is_none()
                && locked.plugin.as_ref() == plugin.as_ref()
                && locked.url.as_ref().is_some_and(|locked_url| {
                    canonicalize_git_url(locked_url) == canonicalize_git_url(url)
                })
                && locked.subpath.as_ref() == subpath.as_ref()
        }
        SourceId::Path {
            canonical,
            subpath,
            plugin,
        } => {
            locked.url.is_none()
                && locked.plugin.as_ref() == plugin.as_ref()
                && locked.subpath.as_ref() == subpath.as_ref()
                && locked
                    .path
                    .as_deref()
                    .is_some_and(|locked_path| paths_equivalent(locked_path, canonical))
        }
    }
}

/// Paths compare as written, except that a Windows separator is a `/`.
fn paths_equivalent(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.replace('\\', "/") == b.replace('\\', "/")
    } else {
        a == b
    }
}

fn identity_string(locked: &LockedDependency) -> String {
    let none = |value: &Option<crate::names::SourceSubpath>| {
        value
            .as_ref()
            .map_or("none".to_string(), |subpath| subpath.to_string())
    };
    let source = match (&locked.url, &locked.path, &locked.subpath) {
        (Some(url), None, Some(subpath)) => format!("git:{url}@{subpath}"),
        (Some(url), None, None) => format!("git:{url}"),
        (None, Some(path), Some(subpath)) => format!("path:{path}@{subpath}"),
        (None, Some(path), None) => format!("path:{path}"),
        (Some(url), Some(path), subpath) => {
            format!(
                "invalid-lock-entry(url={url}, path={path}, subpath={})",
                none(subpath)
            )
        }
        (None, None, subpath) => format!(
            "invalid-lock-entry(url=none, path=none, subpath={})",
            none(subpath)
        ),
    };
    match &locked.plugin {
        Some(plugin) => format!("{source} (plugin {plugin})"),
        None => source,
    }
}
