//! The two version grammars Mars reads and how a version is chosen among a repository's tags.
//!
//! A dependency entry's `version` (see [`parse_constraint`]) and a package's `requires-mars` /
//! `requires-meridian` ([`satisfies_requirement`]) are different grammars: a bare `0.20` is the
//! caret range `^0.20` in the first and `>=0.20.0` in the second. Everything here is pure; the
//! caller supplies the tag list and acts on the choice. [`choose`] is the whole decision: the
//! binary runs its two stages ([`settle`], then [`select`]) around its lock replay, and
//! `selectVersion` of the WASM package runs both at once.

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
pub fn satisfies_requirement(requirement: &str, version: &Version) -> Result<bool, ParseError> {
    let requirement = crate::package::parse_requirement(requirement)
        .map_err(|error| ParseError::Requirement(requirement.to_owned(), error.to_string()))?;
    Ok(requirement.matches(version))
}

/// Version text that is not the kind of version the caller said it was.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("`{0}` is not a semver version: {1}")]
    Version(String, String),
    #[error("`{0}` is not a version requirement: {1}")]
    Requirement(String, String),
    #[error("`{0}` is not a version tag")]
    Tag(String),
}

impl ParseError {
    /// A stable identifier for callers that map failures to their own codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Version(..) => "version.invalid-version",
            Self::Requirement(..) => "version.invalid-requirement",
            Self::Tag(_) => "version.invalid-tag",
        }
    }
}

/// Parse strict semver (no leading `v`).
pub fn parse_version(text: &str) -> Result<Version, ParseError> {
    Version::parse(text).map_err(|error| ParseError::Version(text.to_owned(), error.to_string()))
}

/// [`parse_tag`], where a tag that is not a version is the caller's mistake.
pub fn require_tag(tag: &str) -> Result<Version, ParseError> {
    parse_tag(tag).ok_or_else(|| ParseError::Tag(tag.to_owned()))
}

/// How a chosen version relates to the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionPolicy {
    /// Use the locked version when it is allowed; otherwise the newest allowed.
    PreferLockThenLatest,
    /// The newest allowed version, ignoring the lock.
    LatestOnly,
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
}

impl SelectError {
    /// A stable identifier for callers that map failures to their own codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoSatisfying { .. } => "version.no-satisfying",
        }
    }
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

/// Constraints that cannot both hold on one ref.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettleError {
    #[error("`{first}` requires ref:{first_ref}, but `{second}` requires ref:{second_ref}")]
    PinsDiffer {
        first: String,
        first_ref: String,
        second: String,
        second_ref: String,
    },
    /// A pin names no version, so no range holds at it.
    #[error("`{pinned_by}` requires ref:{reference}, but `{ranged_by}` requires {range}")]
    PinBesideRange {
        pinned_by: String,
        reference: String,
        ranged_by: String,
        range: VersionReq,
    },
}

impl SettleError {
    /// A stable identifier for callers that map failures to their own codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PinsDiffer { .. } => "version.pins-differ",
            Self::PinBesideRange { .. } => "version.pin-beside-range",
        }
    }
}

/// Settle every `(requester, constraint)` on a package: each must hold on the ref it ends at,
/// whether or not it selects anything. A [`Constraint::RefPin`] holds only at that ref, a
/// [`Constraint::Semver`] never at a pin, and [`Constraint::Latest`] anywhere. So the package
/// ends at the one pin if there is one (returned; `None` means [`select`] chooses among the
/// tags), and two pins, or a pin beside a range, refuse. A `Latest` beside a pin holds but
/// drifts: it asked for the newest and gets a fixed ref.
pub fn settle<'a>(constraints: &[(&str, &'a Constraint)]) -> Result<Option<&'a str>, SettleError> {
    let mut pin: Option<(&str, &'a str)> = None;
    for &(requester, constraint) in constraints {
        if let Constraint::RefPin(reference) = constraint {
            match pin {
                Some((first, first_ref)) if first_ref != reference => {
                    return Err(SettleError::PinsDiffer {
                        first: first.to_string(),
                        first_ref: first_ref.to_string(),
                        second: requester.to_string(),
                        second_ref: reference.clone(),
                    });
                }
                Some(_) => {}
                None => pin = Some((requester, reference)),
            }
        }
    }
    let Some((pinned_by, reference)) = pin else {
        return Ok(None);
    };
    for &(requester, constraint) in constraints {
        if let Constraint::Semver(range) = constraint {
            return Err(SettleError::PinBesideRange {
                pinned_by: pinned_by.to_string(),
                reference: reference.to_string(),
                ranged_by: requester.to_string(),
                range: range.clone(),
            });
        }
    }
    Ok(Some(reference))
}

/// The policy [`select`] applies: a [`Constraint::Latest`] asks for the newest, so beside it
/// `PreferLockThenLatest` becomes `LatestOnly`.
pub fn effective_policy(
    policy: SelectionPolicy,
    constraints: &[(&str, &Constraint)],
) -> SelectionPolicy {
    let asks_latest = constraints
        .iter()
        .any(|(_, constraint)| matches!(constraint, Constraint::Latest));
    if policy == SelectionPolicy::PreferLockThenLatest && asks_latest {
        SelectionPolicy::LatestOnly
    } else {
        policy
    }
}

/// Choose one of `available` that meets every semver constraint and is not in `excluded`, and
/// return its index. The newest allowed wins whatever the order of `available`; of equal
/// versions the later one does. `Ok(None)` is a repository with no version tags: the caller
/// takes its default branch. [`Constraint::Latest`] allows any version (see [`effective_policy`]),
/// and a [`Constraint::RefPin`] must have been [settled](settle) already, so it is not read here.
pub fn select(
    available: &[Version],
    constraints: &[(&str, &Constraint)],
    locked: Option<&Version>,
    policy: SelectionPolicy,
    excluded: &HashSet<Version>,
) -> Result<Option<usize>, SelectError> {
    if available.is_empty() {
        return Ok(None);
    }
    let policy = effective_policy(policy, constraints);
    let requirements: Vec<(&str, &VersionReq)> = constraints
        .iter()
        .filter_map(|&(requester, constraint)| match constraint {
            Constraint::Semver(req) => Some((requester, req)),
            _ => None,
        })
        .collect();
    let allowed: Vec<usize> = (0..available.len())
        .filter(|&i| {
            !excluded.contains(&available[i])
                && requirements
                    .iter()
                    .all(|(_, req)| req.matches(&available[i]))
        })
        .collect();

    let Some(newest) = allowed.iter().copied().max_by_key(|&i| (&available[i], i)) else {
        let mut ascending = available.to_vec();
        ascending.sort();
        return Err(SelectError::NoSatisfying {
            constraints: requirements
                .iter()
                .map(|(requester, req)| ((*requester).to_string(), (*req).clone()))
                .collect(),
            available: ascending,
        });
    };

    let locked_allowed =
        locked.and_then(|locked| allowed.iter().copied().find(|&i| available[i] == *locked));
    match policy {
        SelectionPolicy::PreferLockThenLatest => Ok(Some(locked_allowed.unwrap_or(newest))),
        SelectionPolicy::LatestOnly => Ok(Some(newest)),
    }
}

/// What a package ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen<'a> {
    /// A branch or commit every constraint holds at.
    Pin(&'a str),
    /// One of the tags offered.
    Tag(&'a str),
    /// No version tags: the default branch.
    Untagged,
}

/// Why no ref could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChooseError {
    #[error(transparent)]
    Settle(#[from] SettleError),
    #[error(transparent)]
    Select(#[from] SelectError),
}

impl ChooseError {
    /// A stable identifier for callers that map failures to their own codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Settle(error) => error.code(),
            Self::Select(error) => error.code(),
        }
    }
}

/// The whole decision for one package, from every constraint on it and the tags the repository
/// has: [`settle`] first (a pin ends it), then [`select`] among the tags that are versions. A tag
/// that is not one (`nightly`) is ignored, as the binary ignores it when it lists a repository.
pub fn choose<'a>(
    tags: &'a [String],
    constraints: &[(&str, &'a Constraint)],
    locked: Option<&Version>,
    policy: SelectionPolicy,
    excluded: &HashSet<Version>,
) -> Result<Chosen<'a>, ChooseError> {
    if let Some(reference) = settle(constraints)? {
        return Ok(Chosen::Pin(reference));
    }
    let (names, versions): (Vec<&str>, Vec<Version>) = tags
        .iter()
        .filter_map(|tag| Some((tag.as_str(), parse_tag(tag)?)))
        .unzip();
    Ok(select(&versions, constraints, locked, policy, excluded)?
        .map_or(Chosen::Untagged, |index| Chosen::Tag(names[index])))
}
