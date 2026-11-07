//! Config mutation logic for the sync pipeline.
//!
//! Handles applying mutations to `mars.toml` and `mars.local.toml` under the sync lock.

use std::path::PathBuf;

use crate::config::{Config, LocalConfig, OverrideEntry};
use crate::error::{ConfigError, MarsError};
use crate::types::SourceName;
use mars_schema::dependency::{Entry, RawEntry};
use mars_schema::filter::FilterConfig;
use mars_schema::names::{ItemName, RenameMap};

#[derive(Debug, Clone)]
pub enum ConfigMutation {
    /// Add or update a dependency in mars.toml.
    UpsertDependency { name: SourceName, entry: Box<Entry> },
    /// Add or update multiple dependencies in mars.toml atomically under one sync lock.
    BatchUpsert(Vec<(SourceName, Entry)>),
    /// Remove a dependency from mars.toml.
    RemoveDependency { name: SourceName },
    /// Add or update an override in mars.local.toml.
    SetOverride {
        source_name: SourceName,
        local_path: PathBuf,
    },
    /// Set or update a rename mapping for one managed item.
    SetRename {
        source_name: SourceName,
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone)]
pub struct DependencyUpsertChange {
    pub name: SourceName,
    pub already_exists: bool,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub old_filter: Option<FilterConfig>,
    pub new_filter: FilterConfig,
}

/// The error for a dependency name `mars.toml` does not have. A dependency added with a subpath or
/// `--plugin` is named `<repository>/<path or plugin>`, so the bare repository name finds nothing:
/// the names that start with it are listed.
pub(crate) fn dependency_not_found<'a>(
    names: impl Iterator<Item = &'a SourceName>,
    name: &SourceName,
) -> MarsError {
    let prefix = format!("{name}/");
    let similar: Vec<String> = names
        .map(ToString::to_string)
        .filter(|candidate| candidate.starts_with(&prefix))
        .map(|candidate| format!("`{candidate}`"))
        .collect();
    let hint = if similar.is_empty() {
        String::new()
    } else {
        format!("; did you mean {}?", similar.join(", "))
    };
    MarsError::Source {
        source_name: name.to_string(),
        message: format!("dependency `{name}` not found in mars.toml{hint}"),
    }
}

pub(crate) fn apply_mutation(
    config: &mut Config,
    mutation: &ConfigMutation,
) -> Result<Vec<DependencyUpsertChange>, MarsError> {
    match mutation {
        ConfigMutation::UpsertDependency { name, entry } => {
            Ok(vec![apply_dependency_upsert(config, name, entry)])
        }
        ConfigMutation::BatchUpsert(entries) => {
            let mut changes = Vec::with_capacity(entries.len());
            for (name, entry) in entries {
                changes.push(apply_dependency_upsert(config, name, entry));
            }
            Ok(changes)
        }
        ConfigMutation::RemoveDependency { name } => {
            if !config.dependencies.contains_key(name) {
                return Err(dependency_not_found(config.dependencies.keys(), name));
            }
            config.dependencies.shift_remove(name);
            Ok(Vec::new())
        }
        ConfigMutation::SetOverride { .. } => Ok(Vec::new()),
        ConfigMutation::SetRename {
            source_name,
            from,
            to,
        } => {
            if !config.dependencies.contains_key(source_name) {
                return Err(dependency_not_found(
                    config.dependencies.keys(),
                    source_name,
                ));
            }
            let dep = &mut config.dependencies[source_name];
            let rename_map = dep.filter.rename.get_or_insert_with(RenameMap::new);
            rename_map.insert(ItemName::from(from.as_str()), ItemName::from(to.as_str()));
            Ok(Vec::new())
        }
    }
}

pub(crate) fn apply_local_mutation(local: &mut LocalConfig, mutation: &ConfigMutation) {
    match mutation {
        ConfigMutation::SetOverride {
            source_name,
            local_path,
        } => {
            local.overrides.insert(
                source_name.clone(),
                OverrideEntry {
                    path: local_path.clone(),
                },
            );
        }
        ConfigMutation::UpsertDependency { .. }
        | ConfigMutation::BatchUpsert(..)
        | ConfigMutation::RemoveDependency { .. }
        | ConfigMutation::SetRename { .. } => {}
    }
}

fn apply_dependency_upsert(
    config: &mut Config,
    name: &SourceName,
    entry: &Entry,
) -> DependencyUpsertChange {
    if let Some(existing) = config.dependencies.get_mut(name) {
        let old_version = existing.version.clone();
        let old_filter = existing.filter.clone();

        let location = RawEntry::from(entry);
        // A bare `github.com/org/repo` the user wrote stays as written: the entry carries it
        // spelled with `https://`, the same repository.
        let authored_same = existing.url.as_deref().is_some_and(|url| {
            Some(mars_schema::git_url::with_default_scheme(url)) == location.url
        });
        if !authored_same {
            existing.url = location.url;
        }
        existing.path = location.path;
        existing.version = entry.version.clone();
        existing.plugin = entry.plugin.clone();
        // Atomic filter replacement: when any filter field is set on the
        // incoming entry, replace the entire filter config (minus rename).
        // This prevents mixed-mode states like agents + only_skills.
        // When no filter flags are provided (e.g., version bump), preserve existing.
        if entry.filter.has_any_filter() {
            let rename = existing.filter.rename.take();
            existing.filter = entry.filter.clone();
            // Preserve rename — those are set via `mars rename`, not `mars add`
            existing.filter.rename = rename;
        }
        // Never overwrite rename rules from add — those are set via `mars rename`

        DependencyUpsertChange {
            name: name.clone(),
            already_exists: true,
            old_version,
            new_version: existing.version.clone(),
            old_filter: Some(old_filter),
            new_filter: existing.filter.clone(),
        }
    } else {
        config
            .dependencies
            .insert(name.clone(), RawEntry::from(entry));
        DependencyUpsertChange {
            name: name.clone(),
            already_exists: false,
            old_version: None,
            new_version: entry.version.clone(),
            old_filter: None,
            new_filter: entry.filter.clone(),
        }
    }
}

pub(crate) fn is_config_not_found(error: &MarsError) -> bool {
    matches!(error, MarsError::Config(ConfigError::NotFound { .. }))
}
