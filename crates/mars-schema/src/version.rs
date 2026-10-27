//! The two version grammars Mars reads and how a version is chosen among a repository's tags.
//!
//! A dependency entry's `version` (see [`parse_constraint`]) and a package's `requires-mars` /
//! `requires-meridian` ([`satisfies_requirement`]) are different grammars: a bare `0.20` is the
//! caret range `^0.20` in the first and `>=0.20.0` in the second. Everything here is pure; the
//! binary supplies the tag list and acts on the choice.

use std::collections::HashSet;
use std::fmt;

use semver::{Version, VersionReq};

/// A dependency entry's `version`, read.
#[derive(Debug, Clone)]
pub enum Constraint {
    /// Semver requirement (`^1.0`, `>=0.5.0`, `~2.1`, an exact version).
    Semver(VersionReq),
    /// Any version, prefer newest.
    Latest,
    /// Branch or commit pin: no semver resolution.
    RefPin(String),
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constraint::Semver(req) => write!(f, "{req}"),
            Constraint::Latest => write!(f, "latest"),
            Constraint::RefPin(reference) => write!(f, "ref:{reference}"),
        }
    }
}

/// Parse a dependency entry's `version`.
///
/// - `None`, `""` or `"latest"` (any case): [`Constraint::Latest`]
/// - `"v1.2.3"`: exactly that version
/// - `"v2"`: `>=2.0.0, <3.0.0`; `"v2.1"`: `>=2.1.0, <2.2.0`
/// - `">=0.5.0"`, `"^2.0"`, `"~1.2"`, `"0.20"` (a caret range): a semver requirement
/// - anything else: a branch or commit [`Constraint::RefPin`]
pub fn parse_constraint(version: Option<&str>) -> Constraint {
    let version = match version {
        None => return Constraint::Latest,
        Some(v) => v.trim(),
    };

    if version.is_empty() || version.eq_ignore_ascii_case("latest") {
        return Constraint::Latest;
    }

    if let Some(stripped) = version.strip_prefix('v') {
        if let Ok(ver) = Version::parse(stripped) {
            let req = VersionReq::parse(&format!("={ver}")).expect("valid exact req");
            return Constraint::Semver(req);
        }

        // A bound past `u64::MAX` is no range; it falls through to a ref pin.
        if let Ok(major) = stripped.parse::<u64>()
            && let Some(next) = major.checked_add(1)
        {
            let req = VersionReq::parse(&format!(">={major}.0.0, <{next}.0.0"))
                .expect("valid major range req");
            return Constraint::Semver(req);
        }

        let parts: Vec<&str> = stripped.split('.').collect();
        if parts.len() == 2
            && let (Ok(major), Ok(minor)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>())
            && let Some(next) = minor.checked_add(1)
        {
            let req = VersionReq::parse(&format!(">={major}.{minor}.0, <{major}.{next}.0"))
                .expect("valid minor range req");
            return Constraint::Semver(req);
        }
    }

    if let Ok(req) = VersionReq::parse(version) {
        return Constraint::Semver(req);
    }

    Constraint::RefPin(version.to_string())
}

/// Parse a tag name as a semver version tag: one leading `v` is dropped, then the rest must be
/// strict semver. `v1.0.0` and `1.0.0` are versions; `latest` and `nightly-2024` are not.
pub fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// Whether `version` meets a dependency `version`. A [`Constraint::RefPin`] names no version, so
/// nothing meets it.
pub fn satisfies(constraint: &Constraint, version: &Version) -> bool {
    match constraint {
        Constraint::Semver(req) => req.matches(version),
        Constraint::Latest => true,
        Constraint::RefPin(_) => false,
    }
}

/// Whether `version` meets a package's `requires-mars` / `requires-meridian` (a bare `0.20` means
/// `>=0.20.0`). An error is a requirement that is not one.
pub fn satisfies_requirement(requirement: &str, version: &Version) -> Result<bool, semver::Error> {
    Ok(crate::package::parse_requirement(requirement)?.matches(version))
}

/// How a chosen version relates to the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionPolicy {
    /// Use the locked version when it is allowed; otherwise the newest allowed.
    PreferLockThenLatest,
    /// The newest allowed version, ignoring the lock.
    LatestOnly,
    /// The locked version, or an error.
    LockOnly,
}

/// Why no version could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    /// No tag meets every constraint (`requester`, requirement); `available` is ascending.
    #[error("{}", no_satisfying_message(.constraints, .available))]
    NoSatisfying {
        constraints: Vec<(String, VersionReq)>,
        available: Vec<Version>,
    },
    /// [`SelectionPolicy::LockOnly`], and the locked version is excluded, unavailable or fails a constraint.
    #[error("lock version {locked} is incompatible with current constraints or unavailable")]
    LockIncompatible { locked: Version },
    /// [`SelectionPolicy::LockOnly`] with no locked version.
    #[error("no lock version was found")]
    NoLock,
}

fn no_satisfying_message(constraints: &[(String, VersionReq)], available: &[Version]) -> String {
    let wanted: Vec<String> = constraints
        .iter()
        .map(|(requester, req)| format!("  `{requester}` requires {req}"))
        .collect();
    let have: Vec<String> = available.iter().map(Version::to_string).collect();
    format!(
        "no version satisfies all constraints:\n{}\navailable versions: [{}]",
        wanted.join("\n"),
        have.join(", ")
    )
}

/// Choose one of `available` that meets every constraint and is not in `excluded`, and return
/// its index. The newest allowed wins whatever the order of `available`; of equal versions the
/// later one does.
pub fn select(
    available: &[Version],
    constraints: &[(&str, &VersionReq)],
    locked: Option<&Version>,
    policy: SelectionPolicy,
    excluded: &HashSet<Version>,
) -> Result<usize, SelectError> {
    let allowed: Vec<usize> = (0..available.len())
        .filter(|&i| {
            !excluded.contains(&available[i])
                && constraints
                    .iter()
                    .all(|(_, req)| req.matches(&available[i]))
        })
        .collect();

    let Some(newest) = allowed.iter().copied().max_by_key(|&i| (&available[i], i)) else {
        let mut ascending = available.to_vec();
        ascending.sort();
        return Err(SelectError::NoSatisfying {
            constraints: constraints
                .iter()
                .map(|(requester, req)| ((*requester).to_string(), (*req).clone()))
                .collect(),
            available: ascending,
        });
    };

    let locked_allowed =
        locked.and_then(|locked| allowed.iter().copied().find(|&i| available[i] == *locked));
    match policy {
        SelectionPolicy::PreferLockThenLatest => Ok(locked_allowed.unwrap_or(newest)),
        SelectionPolicy::LatestOnly => Ok(newest),
        SelectionPolicy::LockOnly => match (locked_allowed, locked) {
            (Some(index), _) => Ok(index),
            (None, Some(locked)) => Err(SelectError::LockIncompatible {
                locked: locked.clone(),
            }),
            (None, None) => Err(SelectError::NoLock),
        },
    }
}
