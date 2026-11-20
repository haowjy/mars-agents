//! The dependency name a provider's hooks, MCP servers and skill records carry in a lock.
//!
//! Config reserves the prefix, so no package or project dependency can share it.

use std::path::Path;

use mars_schema::dependency::Entry;

use crate::config::{EffectiveDependency, EntrySource};
use crate::error::MarsError;
use crate::types::SourceName;

pub(crate) const SOURCE_PREFIX: &str = "_provided-";

/// `_provided-<provider>`.
pub(crate) fn source_name(provider: &str) -> String {
    format!("{SOURCE_PREFIX}{provider}")
}

pub(crate) fn is_provided_source(name: &str) -> bool {
    name.starts_with(SOURCE_PREFIX)
}

/// The staged hooks and MCP servers of each provider (`.mars/provided/<provider>/`), as the
/// path dependencies sync resolves them through. A host's content takes the same reader, filter
/// and lock paths as a package's, so nothing downstream knows a provider from a dependency.
pub(crate) fn staged_dependencies(
    project_root: &Path,
) -> Result<Vec<(SourceName, EffectiveDependency)>, MarsError> {
    let staging = project_root.join(".mars").join("provided");
    let entries = match std::fs::read_dir(&staging) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut dependencies = Vec::new();
    for entry in entries {
        let entry = entry?;
        let provider = entry.file_name().to_string_lossy().into_owned();
        if !mars_schema::provider::is_provider_name(&provider) || !entry.path().is_dir() {
            continue;
        }
        let dependency = Entry::at_path(entry.path());
        let EntrySource {
            declared_source_id,
            source_id,
            spec,
        } = crate::config::entry_source(&dependency, project_root, None);
        dependencies.push((
            SourceName::from(source_name(&provider)),
            EffectiveDependency {
                declared_source_id,
                source_id,
                spec,
                subpath: None,
                filter: dependency.filter.to_mode(),
                rename: Default::default(),
                dialect: None,
            },
        ));
    }
    dependencies.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(dependencies)
}

/// Whether content of `source_name` is offered to `target`: a package's is offered to every
/// managed target, a host's only to those that take provided content.
pub(crate) fn offered_to(source_name: &str, target: &str) -> bool {
    !is_provided_source(source_name) || crate::harness::registry::takes_provided(target)
}

/// Whether any of the managed targets takes provided content: when none does, a sync has nothing
/// of a host's to read or project.
pub(crate) fn takes_provided_any(managed_targets: &[String]) -> bool {
    managed_targets
        .iter()
        .any(|target| crate::harness::registry::takes_provided(target))
}

/// The keys a host's content gives up: each one claimed both by a provided source and by
/// another. A host's hook, MCP server or skill yields to a package's of the same name on the
/// same target, so the user's sync never fails over a host's content and the package's is
/// installed as if the host had provided none. One rule for all three kinds; a claim is
/// `(key, whether a provided source makes it)`.
pub(crate) fn yielded<K: Eq + std::hash::Hash>(
    claims: impl IntoIterator<Item = (K, bool)>,
) -> std::collections::HashSet<K> {
    // (a provided source claims it, another source does)
    let mut seen: std::collections::HashMap<K, (bool, bool)> = std::collections::HashMap::new();
    for (key, by_provided) in claims {
        let held = seen.entry(key).or_default();
        if by_provided {
            held.0 = true;
        } else {
            held.1 = true;
        }
    }
    seen.into_iter()
        .filter(|(_, (provided, other))| *provided && *other)
        .map(|(key, _)| key)
        .collect()
}

/// The host a provided source stands for, for a message to the user: `meridian`.
pub(crate) fn provider_label(source_name: &str) -> &str {
    source_name
        .strip_prefix(SOURCE_PREFIX)
        .unwrap_or(source_name)
}

/// `path` for a message. A provided source's files sit in a staging directory the user never
/// sees, so they read as `<host>/<path under the bundle>`.
pub(crate) fn display_path(source_name: &str, root: &Path, path: &Path) -> String {
    if is_provided_source(source_name) {
        let relative = path.strip_prefix(root).unwrap_or(path);
        Path::new(provider_label(source_name))
            .join(relative)
            .display()
            .to_string()
    } else {
        path.display().to_string()
    }
}
