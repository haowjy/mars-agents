//! The version `mars upgrade` would move a dependency to, for the hints that advertise it
//! (`mars sync`'s "upgrade available" and `mars outdated`).

use std::collections::HashSet;

use mars_schema::version::{Constraint, SelectionPolicy, select, settle};
use semver::Version;

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::source::AvailableVersion;
use crate::types::{SourceName, SourceUrl};

use super::requires::check_package_requirements;
use super::{ResolveOptions, SourceProvider};

/// The newest of `available` newer than `floor` (the version in use) that `constraints` allow and
/// whose `requires-*` the running engines meet, which is what `mars upgrade` picks. A newer tag
/// that needs a newer Mars is skipped, as the resolver skips it. `Ok(None)` when a pin settles the
/// package or nothing newer qualifies; `Err` when a candidate's manifest cannot be read, because an
/// unverified version is not advertised.
///
/// Each candidate tried costs one fetch of its tree. The scan stops at `floor`, so a package that
/// is up to date fetches nothing and one whose newer tags are all gated fetches only those.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upgrade_candidate<'a>(
    name: &SourceName,
    url: &SourceUrl,
    subpath: Option<&mars_schema::names::SourceSubpath>,
    available: &'a [AvailableVersion],
    floor: Option<&Version>,
    constraints: &[(&str, &Constraint)],
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<Option<&'a AvailableVersion>, MarsError> {
    if !matches!(settle(constraints), Ok(None)) {
        return Ok(None);
    }
    let versions: Vec<Version> = available.iter().map(|av| av.version.clone()).collect();
    let mut excluded = HashSet::new();
    loop {
        let Ok(Some(index)) = select(
            &versions,
            constraints,
            None,
            SelectionPolicy::LatestOnly,
            &excluded,
        ) else {
            return Ok(None);
        };
        let candidate = &available[index];
        if floor.is_some_and(|floor| candidate.version <= *floor) {
            return Ok(None);
        }
        if engines_accept(name, url, subpath, candidate, provider, options, diag)? {
            return Ok(Some(candidate));
        }
        excluded.insert(candidate.version.clone());
    }
}

/// Whether the engines meet the candidate's `requires-*`.
fn engines_accept(
    name: &SourceName,
    url: &SourceUrl,
    subpath: Option<&mars_schema::names::SourceSubpath>,
    candidate: &AvailableVersion,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    let fetched = provider.fetch_git_version(url, candidate, name.as_ref(), None, diag)?;
    let root = super::path::apply_subpath(name, &fetched.tree_path, subpath)?;
    let text = match std::fs::read_to_string(root.package_root.join("mars.toml")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    match mars_schema::manifest::read(&text).package.fields {
        Some(package) => Ok(check_package_requirements(&package, options)?.is_empty()),
        None => Ok(true),
    }
}
