//! The version `mars upgrade` would move a dependency to, for the hints that advertise it
//! (`mars sync`'s "upgrade available" and `mars outdated`).

use std::collections::HashSet;

use mars_schema::version::{Constraint, SelectionPolicy, select, settle};
use semver::Version;

use crate::diagnostic::DiagnosticCollector;
use crate::source::AvailableVersion;
use crate::types::{SourceName, SourceUrl};

use super::requires::check_package_requirements;
use super::{ResolveOptions, SourceProvider};

/// The newest of `available` that `constraints` allow and whose `requires-*` the running engines
/// meet, which is what `mars upgrade` picks. A newer tag that needs a newer Mars is skipped, as
/// the resolver skips it. `None` when a pin settles the package, nothing qualifies, or a
/// candidate's manifest cannot be read: an unverified version is not advertised.
///
/// Reading a candidate's manifest fetches its tree (cached once fetched), so the cost is one
/// fetch per candidate tried, not per call.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upgrade_candidate<'a>(
    name: &SourceName,
    url: &SourceUrl,
    subpath: Option<&mars_schema::names::SourceSubpath>,
    available: &'a [AvailableVersion],
    constraints: &[(&str, &Constraint)],
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Option<&'a AvailableVersion> {
    if !matches!(settle(constraints), Ok(None)) {
        return None;
    }
    let versions: Vec<Version> = available.iter().map(|av| av.version.clone()).collect();
    let mut excluded = HashSet::new();
    loop {
        let candidate = &available[select(
            &versions,
            constraints,
            None,
            SelectionPolicy::LatestOnly,
            &excluded,
        )
        .ok()??];
        if engines_accept(name, url, subpath, candidate, provider, options, diag)? {
            return Some(candidate);
        }
        excluded.insert(candidate.version.clone());
    }
}

/// Whether the engines meet the candidate's `requires-*`; `None` if its manifest cannot be read.
fn engines_accept(
    name: &SourceName,
    url: &SourceUrl,
    subpath: Option<&mars_schema::names::SourceSubpath>,
    candidate: &AvailableVersion,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Option<bool> {
    let fetched = provider
        .fetch_git_version(url, candidate, name.as_ref(), None, diag)
        .ok()?;
    let root = super::path::apply_subpath(name, &fetched.tree_path, subpath).ok()?;
    let text = match std::fs::read_to_string(root.package_root.join("mars.toml")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(true),
        Err(_) => return None,
    };
    match mars_schema::manifest::read(&text).package.fields {
        Some(package) => Some(
            check_package_requirements(&package, options)
                .ok()?
                .is_empty(),
        ),
        None => Some(true),
    }
}
