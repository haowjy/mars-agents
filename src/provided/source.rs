//! The dependency name a provider's hooks, MCP servers and skill records carry in a lock.
//!
//! Config reserves the prefix, so no package or project dependency can share it.

use std::path::Path;

use mars_schema::dependency::Entry;

use crate::config::{EffectiveDependency, EntrySource};
use crate::error::MarsError;
use crate::types::SourceName;

use crate::layer::HOST_PREFIX as SOURCE_PREFIX;

/// `_provided-<provider>`.
pub(crate) fn source_name(provider: &str) -> String {
    format!("{SOURCE_PREFIX}{provider}")
}

/// The staged hooks and MCP servers of each provider (`.mars/provided/<provider>/`), as the
/// path dependencies sync resolves them through. A host's content takes the same reader, filter
/// and lock paths as a package's, so nothing downstream knows a provider from a dependency.
pub(crate) fn staged_dependencies(
    layout: &crate::layout::Layout,
) -> Result<Vec<(SourceName, EffectiveDependency)>, MarsError> {
    let staging = layout.store().join("provided");
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
        } = crate::config::entry_source(&dependency, layout.root(), None);
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

/// Whether any of the managed targets takes provided content: when none does, a sync has nothing
/// of a host's to read or project.
pub(crate) fn takes_provided_any(managed_targets: &[String]) -> bool {
    managed_targets
        .iter()
        .any(|target| crate::harness::registry::takes_provided(target))
}

/// `path` for a message. A provided source's files sit in a staging directory the user never
/// sees, so they read as `<host>/<path under the bundle>`.
pub(crate) fn display_path(source_name: &str, root: &Path, path: &Path) -> String {
    if crate::layer::is_host(source_name) {
        let relative = path.strip_prefix(root).unwrap_or(path);
        Path::new(&crate::layer::source_label(source_name))
            .join(relative)
            .display()
            .to_string()
    } else {
        path.display().to_string()
    }
}
