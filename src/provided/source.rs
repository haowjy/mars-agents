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

/// The host a provided source stands for, for a message to the user: `meridian`.
pub(crate) fn provider_label(source_name: &str) -> &str {
    source_name
        .strip_prefix(SOURCE_PREFIX)
        .unwrap_or(source_name)
}
