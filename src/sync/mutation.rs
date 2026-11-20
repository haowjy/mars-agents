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
    /// Name a target in `settings.targets` of mars.toml, or of mars.local.toml when `local`, so
    /// the sync that follows installs into it.
    LinkTarget { target: String, local: bool },
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

/// The error for a dependency name `place` (`mars.toml`, the resolved graph) does not have. A dependency added with a subpath or
/// `--plugin` is named `<repository>/<path or plugin>`, so the bare repository name finds nothing:
/// the names that start with it are listed.
pub(crate) fn dependency_not_found<'a>(
    names: impl Iterator<Item = &'a SourceName>,
    name: &SourceName,
    place: &str,
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
        message: format!("dependency `{name}` not found in {place}{hint}"),
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
                return Err(dependency_not_found(
                    config.dependencies.keys(),
                    name,
                    "mars.toml",
                ));
            }
            config.dependencies.shift_remove(name);
            Ok(Vec::new())
        }
        ConfigMutation::SetOverride { .. } => Ok(Vec::new()),
        ConfigMutation::LinkTarget {
            target,
            local: false,
        } => {
            let mut targets = crate::config::targets::managed_targets(&config.settings);
            if !targets.contains(target) {
                targets.push(target.clone());
            }
            // The key is retired; leaving it beside the `targets` just written would only warn.
            config.settings.managed_root = None;
            config.settings.targets = Some(targets);
            Ok(Vec::new())
        }
        ConfigMutation::LinkTarget { local: true, .. } => Ok(Vec::new()),
        ConfigMutation::SetRename {
            source_name,
            from,
            to,
        } => {
            if !config.dependencies.contains_key(source_name) {
                return Err(dependency_not_found(
                    config.dependencies.keys(),
                    source_name,
                    "mars.toml",
                ));
            }
            let dep = &mut config.dependencies[source_name];
            let rename_map = dep.filter.rename.get_or_insert_with(RenameMap::new);
            rename_map.insert(ItemName::from(from.as_str()), ItemName::from(to.as_str()));
            Ok(Vec::new())
        }
    }
}

/// The first Mars where a local `targets` list adds to the project's. Mars 0.38.0 reads it as
/// the whole list and, under its advised `--force`, rewrites the shared lock to match. A build
/// before it is a development build of it, which must not stop itself.
fn adding_targets_floor() -> semver::Version {
    let running =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver");
    running.min(semver::Version::new(0, 39, 0))
}

/// Which files `mars link` changes: `(mars.toml, mars.local.toml)`. The project file is the
/// target's home unless `--local` asked for this machine only; a target `mars.toml` already
/// names stays there, and a copy in `mars.local.toml` goes.
pub(crate) fn link_changes(
    config: &Config,
    local: &LocalConfig,
    target: &str,
    local_only: bool,
) -> (bool, bool) {
    use crate::config::targets::{names_target, project_targets};
    if local_only {
        let add = !project_targets(config).contains(target)
            && !names_target(local.settings.targets.as_deref(), target);
        (false, add)
    } else {
        (
            !project_targets(config).contains(target),
            names_target(local.settings.targets.as_deref(), target),
        )
    }
}

pub(crate) fn apply_local_mutation(
    local: &mut LocalConfig,
    config: &Config,
    user: &crate::config::UserConfig,
    project_root: &std::path::Path,
    mutation: &ConfigMutation,
) -> Result<(), MarsError> {
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
        ConfigMutation::LinkTarget {
            target,
            local: false,
        } => {
            crate::config::targets::unset_target(&mut local.settings.targets, target);
        }
        ConfigMutation::LinkTarget {
            target,
            local: true,
        } => {
            if link_changes(config, local, target, true).1 {
                let floor = adding_targets_floor();
                let (effective, _) = crate::config::merge_with_root(
                    config.clone(),
                    local.clone(),
                    user,
                    project_root,
                )?;
                if !mars_schema::package::requirement_has_floor(
                    effective.settings.min_mars_version.as_deref(),
                    &floor,
                ) {
                    local.settings.min_mars_version = Some(floor.to_string());
                }
                local
                    .settings
                    .targets
                    .get_or_insert_with(Vec::new)
                    .push(target.clone());
            }
        }
        ConfigMutation::UpsertDependency { .. }
        | ConfigMutation::BatchUpsert(..)
        | ConfigMutation::RemoveDependency { .. }
        | ConfigMutation::SetRename { .. } => {}
    }
    Ok(())
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
