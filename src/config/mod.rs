// qa-validated: harness-order-settings-audit

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticLevel};
use crate::error::{ConfigError, MarsError};
use crate::types::{SourceId, SourceName, SourceOrigin, SourceUrl};
pub use mars_schema::config::{
    AgentCopyConfig, AgentEmission, AgentOverlay, Config, FanoutConfig, MeridianSettings, Settings,
    SkillOverlay,
};
use mars_schema::dependency::{Entry, Source, Strictness};
use mars_schema::diagnostic::SchemaDiagnostic;
use mars_schema::dialect::Dialect;
use mars_schema::filter::FilterMode;
use mars_schema::keys;
use mars_schema::manifest::ManifestRead;
use mars_schema::model_policy::ModelPolicyRule;
use mars_schema::names::{RenameMap, SourceSubpath};
use mars_schema::network::{NetworkHost, NetworkPolicy};
pub use mars_schema::package::PackageMetadata as PackageInfo;
use mars_schema::permission_set::PermissionSet;
use mars_schema::tool_rule_map::ToolRuleMap;

pub mod layering;
pub mod migrations;
pub mod routing_settings;
mod save;
pub mod targets;
mod unknown;
mod user;

pub use user::UserConfig;
pub(crate) use user::moved_warning;

use layering::{ComposedLayers, compose_layers};
pub use layering::{overlay_agent_overlays, overlay_models_replace_by_key, overlay_skill_overlays};

pub(crate) fn validate_plugin_dir(dir: &Path) -> Result<(), ConfigError> {
    const SOURCE_DIRS: &[&str] = &["agents", "skills", "hooks", "bootstrap"];

    if dir.as_os_str().is_empty() {
        return Err(ConfigError::Invalid {
            message: "mars.toml [plugin].dir cannot be empty".to_string(),
        });
    }
    for component in dir.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(ConfigError::Invalid {
                message: format!(
                    "mars.toml [plugin].dir `{}` must be a relative path inside the package without `.` or `..` components",
                    dir.display()
                ),
            });
        };
        let name = name.to_str().ok_or_else(|| ConfigError::Invalid {
            message: "mars.toml [plugin].dir must be valid Unicode".to_string(),
        })?;
        if name.starts_with('.') {
            return Err(ConfigError::Invalid {
                message: format!(
                    "mars.toml [plugin].dir `{}` cannot use a dot directory",
                    dir.display()
                ),
            });
        }
        if SOURCE_DIRS.contains(&name) {
            return Err(ConfigError::Invalid {
                message: format!(
                    "mars.toml [plugin].dir `{}` cannot use the source convention directory `{name}`",
                    dir.display()
                ),
            });
        }
    }
    Ok(())
}

mod toml_path_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::path::{Path, PathBuf};

    pub fn serialize<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = path.to_string_lossy().replace('\\', "/");
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PathBuf::from(s))
    }
}

/// Source-manifest view extracted from mars.toml.
///
/// In source repositories, `mars.toml` may include `[package]` +
/// `[dependencies]` only, or coexist with consumer sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub package: PackageInfo,
    pub dependencies: IndexMap<String, Entry>,
    pub models: IndexMap<String, crate::models::ModelAlias>,
}

/// Dev override config (mars.local.toml).
///
/// Gitignored — each developer can work with local checkouts while
/// production config points at git.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LocalConfig {
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub overrides: IndexMap<SourceName, OverrideEntry>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub models: IndexMap<String, crate::models::ModelAlias>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub agents: IndexMap<String, AgentOverlay>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub skills: IndexMap<String, SkillOverlay>,
    #[serde(default, skip_serializing_if = "LocalSettings::is_empty")]
    pub settings: LocalSettings,
    /// As in `Config`.
    #[serde(skip)]
    pub(crate) unknown_keys: Vec<SchemaDiagnostic>,
    /// The Mars the file's tool rules need, set when the file is read.
    #[serde(skip)]
    pub(crate) floor: Option<keys::Floor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LocalSettings {
    /// As in `Settings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_cache_ttl_hours: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mars_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(
        default,
        alias = "default-network",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_network: Option<NetworkPolicy>,
    #[serde(
        default,
        rename = "network-allow",
        alias = "network_allow",
        skip_serializing_if = "Option::is_none"
    )]
    pub network_allow: Option<PermissionSet<NetworkHost>>,
    #[serde(
        default,
        rename = "network-deny",
        alias = "network_deny",
        skip_serializing_if = "Option::is_none"
    )]
    pub network_deny: Option<PermissionSet<NetworkHost>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolRuleMap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_providers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_emission: Option<AgentEmission>,
    #[serde(default, skip_serializing_if = "MeridianSettings::is_empty")]
    pub meridian: MeridianSettings,
    #[serde(default, rename = "model-policies")]
    pub model_policies: Option<Vec<ModelPolicyRule>>,
}

/// Dev override — local path swap for a git source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverrideEntry {
    #[serde(with = "toml_path_serde")]
    pub path: PathBuf,
}

/// Resolved source specification after merging config and overrides.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    Git(GitSpec),
    Path(PathBuf),
}

/// Git source specification preserved when overrides are active.
#[derive(Debug, Clone)]
pub struct GitSpec {
    pub url: SourceUrl,
    pub version: Option<String>,
}

/// Effective configuration after merging mars.toml and mars.local.toml.
///
/// This is what the rest of the pipeline operates on.
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub dependencies: IndexMap<SourceName, EffectiveDependency>,
    pub settings: Settings,
    /// Model aliases from the user config, mars.toml and mars.local.toml, composed once.
    pub models: IndexMap<String, crate::models::ModelAlias>,
    /// Agent overlays from the user config, mars.toml and mars.local.toml, composed once.
    pub agents: IndexMap<String, AgentOverlay>,
    /// Skill overlays from the user config, mars.toml and mars.local.toml (carried for staging).
    pub skills: IndexMap<String, SkillOverlay>,
}

/// Layered project-level config used by routing and policy consumers.
///
/// Precedence: project config < project-local overrides.
#[derive(Debug, Clone, Default)]
pub struct EffectiveProjectConfig {
    pub settings: Settings,
    pub target_source: targets::TargetSource,
    pub models: IndexMap<String, crate::models::ModelAlias>,
    pub agents: IndexMap<String, AgentOverlay>,
    /// What the files hold that Mars noticed: unread keys (`Info`) and a missing version floor.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedProjectConfig {
    pub(crate) config: Config,
    pub(crate) local: LocalConfig,
    pub(crate) user: UserConfig,
    pub(crate) effective: EffectiveProjectConfig,
}

/// A dependency's declared configuration and effective source after local overrides.
#[derive(Debug, Clone)]
pub struct EffectiveDependency {
    /// Identity from the dependency entry in `mars.toml`, before local overrides.
    pub declared_source_id: SourceId,
    /// Identity used to fetch and resolve the dependency.
    pub source_id: SourceId,
    pub spec: SourceSpec,
    pub subpath: Option<SourceSubpath>,
    pub filter: FilterMode,
    pub rename: RenameMap,
    pub dialect: Option<Dialect>,
}

use crate::layout::{CONFIG_FILE, LOCAL_CONFIG_FILE, Layout};

pub(super) fn read_error(path: &Path, source: std::io::Error) -> MarsError {
    MarsError::Io {
        operation: "read config".to_string(),
        path: path.to_path_buf(),
        source,
    }
}

impl LocalConfig {
    /// The `[overrides]` as absolute paths, relative ones resolved against the project root.
    pub(crate) fn source_overrides(&self, root: &Path) -> IndexMap<SourceName, PathBuf> {
        self.overrides
            .iter()
            .map(|(name, entry)| {
                let path = if entry.path.is_absolute() {
                    entry.path.clone()
                } else {
                    root.join(&entry.path)
                };
                (name.clone(), path)
            })
            .collect()
    }
}

/// Load the root's `mars.toml`.
pub fn load(layout: &Layout) -> Result<Config, MarsError> {
    let path = layout.manifest();
    let content = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::NotFound { path: path.clone() }.into()
        } else {
            read_error(&path, e)
        }
    })?;
    parse_config(&content, &path)
}

pub(crate) fn parse_config(content: &str, path: &Path) -> Result<Config, MarsError> {
    let read = mars_schema::config::read_consumer(content);
    if let Some(error) = read.parse_error {
        return Err(parse_failed(path, &error));
    }
    refuse(path, &read.issues)?;
    Ok(read.config)
}

fn parse_failed(path: &Path, error: &toml::de::Error) -> MarsError {
    ConfigError::Invalid {
        message: format!("failed to parse {}: {error}", path.display()),
    }
    .into()
}

fn read_file<T: DeserializeOwned>(
    content: &str,
    path: &Path,
    document: keys::Document,
) -> Result<mars_schema::config::Read<T>, MarsError> {
    mars_schema::config::read(content, document).map_err(|error| parse_failed(path, &error))
}

/// Interpret consumer-owned manifest sections from bytes already normalized by
/// `mars-schema`. Schema diagnostics are supplied by the prepared result and are
/// deliberately not regenerated here.
pub(crate) fn parse_prepared_manifest(
    content: &str,
    authored_path: &Path,
    reading: &ManifestRead,
) -> Result<Option<Manifest>, MarsError> {
    let models = mars_schema::config::read_models(content).map_err(|error| {
        crate::error::ConfigError::Invalid {
            message: format!("failed to parse {}: {error}", authored_path.display()),
        }
    })?;
    let Some(package) = reading.package.fields.clone() else {
        return Ok(None);
    };
    Ok(Some(Manifest {
        package,
        dependencies: reading.dependencies.dependencies.clone(),
        models,
    }))
}

/// Read a `mars.local.toml`-shaped file: the project's local file and the user `config.toml`.
/// `file` names it in what the reader reports.
fn parse_local(
    content: &str,
    path: &Path,
    document: keys::Document,
) -> Result<LocalConfig, MarsError> {
    let mars_schema::config::Read {
        mut value,
        unknown_keys,
        floor,
    } = read_file::<LocalConfig>(content, path, document)?;
    refuse(
        path,
        &mars_schema::config::settings_refusals(
            value.settings.tools.as_ref(),
            value.settings.min_mars_version.as_deref(),
        ),
    )?;
    value.unknown_keys = unknown_keys;
    value.floor = floor;
    Ok(value)
}

/// Load mars.local.toml (returns Default if absent).
pub fn load_local(layout: &Layout) -> Result<LocalConfig, MarsError> {
    let path = layout.local_config();
    match std::fs::read_to_string(&path) {
        Ok(content) => parse_local(&content, &path, keys::Document::Local),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LocalConfig::default()),
        Err(e) => Err(read_error(&path, e)),
    }
}

/// Base model precedence for the overlay+profile layer shared by native emission
/// and launch-bundle: `overlay.model` wins over `profile.model`. Settings-level
/// defaults (e.g. `settings.default_model`) are layered on by launch-bundle only.
pub fn overlay_then_profile_model<'a>(
    overlay: Option<&'a AgentOverlay>,
    profile_model: Option<&'a str>,
) -> Option<&'a str> {
    overlay
        .and_then(|entry| entry.model.as_deref())
        .or(profile_model)
}

/// Model-policy precedence for the overlay+profile layer: overlay policies first,
/// then profile policies. Each item carries `is_overlay` (its source layer) and the
/// index within that layer's own list, so callers can index back into the originating
/// slice. Launch-bundle appends `settings.model_policies` after this iterator; native
/// emission consumes it directly (settings policies stay launch-bundle/spawn-only).
pub fn overlay_then_profile_policies<'a>(
    overlay: Option<&'a AgentOverlay>,
    profile_policies: &'a [ModelPolicyRule],
) -> impl Iterator<Item = (bool, usize, &'a ModelPolicyRule)> + 'a {
    overlay
        .into_iter()
        .flat_map(|entry| {
            entry
                .model_policies
                .as_deref()
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(index, rule)| (true, index, rule))
        })
        .chain(
            profile_policies
                .iter()
                .enumerate()
                .map(|(index, rule)| (false, index, rule)),
        )
}

pub fn load_config_with_local(layout: &Layout) -> Result<(Config, LocalConfig), MarsError> {
    let config = load(layout)?;
    let local = load_local(layout)?;
    Ok((config, local))
}

fn effective_project_config(
    config: &Config,
    local: &LocalConfig,
    user: &UserConfig,
    layout: Option<&Layout>,
) -> EffectiveProjectConfig {
    let ComposedLayers {
        settings,
        mut target_source,
        models,
        agents,
        ..
    } = compose_layers(config, local, &user.local);
    target_source.path = layout.and_then(|layout| match target_source.origin {
        targets::TargetOrigin::Project => Some(layout.manifest()),
        targets::TargetOrigin::Local => Some(layout.local_config()),
        targets::TargetOrigin::Unset => None,
    });
    EffectiveProjectConfig {
        settings,
        target_source,
        models,
        agents,
        diagnostics: project_config_diagnostics(config, local, user),
    }
}

/// The first refusal a file carries, as the file's own error.
fn refuse(file: &Path, issues: &[mars_schema::config::ConfigIssue]) -> Result<(), MarsError> {
    match issues.first() {
        Some(issue) => Err(ConfigError::Invalid {
            message: format!("{}: {}", file.display(), issue.message),
        }
        .into()),
        None => Ok(()),
    }
}

/// What a file of the project needs of Mars beyond the `min_mars_version` it states: its own
/// floor, from the tool rules it writes. Each file is judged alone, since `mars.toml` is shared
/// with people whose Mars never reads the local file. A floor at or below the first Mars that
/// reads `min_mars_version` is not worth a line: no Mars would act on it.
fn floor_warning(
    file: &str,
    floor: Option<&keys::Floor>,
    stated: Option<&str>,
) -> Option<Diagnostic> {
    let floor = floor.filter(|floor| floor.since > keys::MIN_MARS_VERSION_SINCE)?;
    if mars_schema::package::requirement_has_floor(stated, &floor.since.version()) {
        return None;
    }
    let stated_text = match stated {
        Some(raw) => format!("`min_mars_version = \"{raw}\"` admits an older one"),
        None => "no `min_mars_version` says so".to_string(),
    };
    Some(Diagnostic {
        level: DiagnosticLevel::Warning,
        code: "min-mars-version-low".into(),
        context: None,
        category: Some(DiagnosticCategory::Validation),
        message: format!(
            "{file}: {} needs Mars {}, but {stated_text}: write `min_mars_version = \"{}\"` under `[settings]` so a Mars that reads it stops instead of misreading the file (Mars before {} does not read it)",
            floor.at,
            floor.since,
            floor.since,
            keys::MIN_MARS_VERSION_SINCE
        ),
    })
}

fn project_config_diagnostics(
    config: &Config,
    local: &LocalConfig,
    user: &UserConfig,
) -> Vec<Diagnostic> {
    let user_file = user.path.display().to_string();
    // The user's own file stays a note; the project's files warn (`unknown::warning`).
    let unknown_keys =
        user.local
            .unknown_keys
            .iter()
            .map(|note| Diagnostic::from_schema(note, format!("{user_file}: {}", note.message)))
            .chain(
                config
                    .unknown_keys
                    .iter()
                    .map(|note| unknown::warning::<Config, Settings>(note, CONFIG_FILE)),
            )
            .chain(local.unknown_keys.iter().map(|note| {
                unknown::warning::<LocalConfig, LocalSettings>(note, LOCAL_CONFIG_FILE)
            }));
    let floors = [
        (
            user_file.as_str(),
            user.local.floor.as_ref(),
            user.local.settings.min_mars_version.as_deref(),
        ),
        (
            CONFIG_FILE,
            config.floor.as_ref(),
            config.settings.min_mars_version.as_deref(),
        ),
        (
            LOCAL_CONFIG_FILE,
            local.floor.as_ref(),
            local.settings.min_mars_version.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(file, floor, stated)| floor_warning(file, floor, stated));
    let moved = user
        .moved_to
        .as_ref()
        .map(|moved_to| user::moved_warning(&user.path, moved_to));
    unknown_keys.chain(floors).chain(moved).collect()
}

pub fn load_effective_project_config(layout: &Layout) -> Result<EffectiveProjectConfig, MarsError> {
    Ok(load_project_config_layers(layout)?.effective)
}

/// The effective config outside a Mars project: the user's `config.toml` alone.
pub fn load_user_effective_project_config() -> Result<EffectiveProjectConfig, MarsError> {
    Ok(effective_project_config(
        &Config::default(),
        &LocalConfig::default(),
        &user::load()?,
        None,
    ))
}

/// The user's `config.toml` layer (empty when the file is absent).
pub(crate) fn load_user_config() -> Result<UserConfig, MarsError> {
    user::load()
}

pub(crate) fn load_project_config_layers(
    layout: &Layout,
) -> Result<LoadedProjectConfig, MarsError> {
    let (config, local) = load_config_with_local(layout)?;
    let user = user::load()?;
    let effective = effective_project_config(&config, &local, &user, Some(layout));
    Ok(LoadedProjectConfig {
        config,
        local,
        user,
        effective,
    })
}

/// Merge config + local overrides into `EffectiveConfig`, resolving path dependencies
/// relative to `root`.
///
/// Validates:
/// - Each source has `url` XOR `path` (not both, not neither)
/// - Each source uses either include filters (`agents`/`skills`) or `exclude`, not both
/// - Collects diagnostics if an override references a source name not in config
pub fn merge_with_root(
    config: Config,
    local: LocalConfig,
    user: &UserConfig,
    root: &Path,
) -> Result<(EffectiveConfig, Vec<Diagnostic>), MarsError> {
    let layers = compose_layers(&config, &local, &user.local);
    let merged_settings = layers.settings;
    let mut dependencies = IndexMap::new();
    let mut diagnostics = project_config_diagnostics(&config, &local, user);
    let local_source_name = SourceOrigin::LocalPackage.to_string();

    let links = targets::effective_links(&merged_settings);
    diagnostics.extend(targets::target_advisories(
        &links.managed_targets(),
        &links.retired_targets(),
    ));

    // Process both regular and local dependencies into the same effective map.
    // Local deps are installed locally but not exported to consumers via manifest.
    let all_deps = config
        .dependencies
        .iter()
        .chain(config.local_dependencies.iter());

    for (name, entry) in all_deps {
        // Reject reserved name
        if name.as_ref() == local_source_name.as_str() {
            return Err(ConfigError::Invalid {
                message: "dependency name `_self` is reserved for local package items".into(),
            }
            .into());
        }

        if crate::provided::is_provided_source(name.as_ref()) {
            return Err(ConfigError::Invalid {
                message: format!(
                    "dependency name `{name}` is reserved: names starting `_provided-` are what a host's provided content is recorded under"
                ),
            }
            .into());
        }

        // Reject duplicate names across sections
        if dependencies.contains_key(name) {
            return Err(ConfigError::Invalid {
                message: format!(
                    "dependency `{name}` appears in both [dependencies] and [local-dependencies]"
                ),
            }
            .into());
        }

        // The file holds entries as written; the rules every reader shares apply here, where
        // an entry is used, so `mars remove` can still repair a bad one.
        let (entry, _) = Entry::read(name.as_ref(), entry.clone(), Strictness::Consumer)
            .map_err(ConfigError::from)?;

        let filter = entry.filter.to_mode();
        let rename = entry.filter.rename.clone().unwrap_or_default();
        let subpath = entry.subpath.clone();
        let dialect = entry.dialect;
        let EntrySource {
            declared_source_id,
            source_id: effective_source_id,
            spec,
        } = entry_source(
            &entry,
            root,
            local.overrides.get(name).map(|ov| ov.path.as_path()),
        );

        dependencies.insert(
            name.clone(),
            EffectiveDependency {
                declared_source_id,
                source_id: effective_source_id,
                spec,
                subpath,
                filter,
                rename,
                dialect,
            },
        );
    }

    Ok((
        EffectiveConfig {
            dependencies,
            settings: merged_settings,
            models: layers.models,
            agents: layers.agents,
            skills: layers.skills,
        },
        diagnostics,
    ))
}

/// Where an entry's package comes from, whichever manifest holds the entry.
pub(crate) struct EntrySource {
    /// The identity the entry declares, before any local override.
    pub(crate) declared_source_id: SourceId,
    /// The identity fetched this run.
    pub(crate) source_id: SourceId,
    /// What to fetch: the entry's source, or a local override's path.
    pub(crate) spec: SourceSpec,
}

/// Read an entry of the manifest in `base_dir` (a relative `path` is relative to it), with the
/// local path override for its name if there is one.
pub(crate) fn entry_source(
    entry: &Entry,
    base_dir: &Path,
    override_path: Option<&Path>,
) -> EntrySource {
    let declared = match &entry.source {
        Source::Git { url } => SourceSpec::Git(GitSpec {
            url: SourceUrl::from(url.as_str()),
            version: entry.version.clone(),
        }),
        Source::Path { path } => SourceSpec::Path(if path.is_absolute() {
            path.clone()
        } else {
            base_dir.join(path)
        }),
    };
    let declared_source_id = source_id_for_spec(
        base_dir,
        &declared,
        entry.subpath.clone(),
        entry.plugin.as_deref(),
    );
    let spec = override_path.map_or(declared, |path| SourceSpec::Path(path.to_path_buf()));
    let source_id = source_id_for_spec(
        base_dir,
        &spec,
        entry.subpath.clone(),
        entry.plugin.as_deref(),
    );
    EntrySource {
        declared_source_id,
        source_id,
        spec,
    }
}

pub(crate) fn source_id_for_spec(
    root: &Path,
    spec: &SourceSpec,
    subpath: Option<SourceSubpath>,
    plugin: Option<&str>,
) -> SourceId {
    let id = match spec {
        SourceSpec::Git(git) => {
            let canonical_url =
                SourceUrl::from(mars_schema::git_url::canonicalize_git_url(git.url.as_ref()));
            SourceId::git_with_subpath(canonical_url, subpath)
        }
        SourceSpec::Path(path) => SourceId::path_with_subpath(root, path, subpath.clone())
            .unwrap_or_else(|_| {
                let canonical = if path.is_absolute() {
                    path.clone()
                } else {
                    root.join(path)
                };
                SourceId::Path {
                    canonical,
                    subpath,
                    plugin: None,
                }
            }),
    };
    id.with_plugin(plugin.map(str::to_owned))
}

/// Write mars.toml, changing only what differs from the file as it stands.
pub fn save(layout: &Layout, config: &Config) -> Result<(), MarsError> {
    let path = layout.manifest();
    save::save_document(&path, config, |text| parse_config(text, &path))
}

/// Write mars.local.toml, changing only what differs from the file as it stands.
pub fn save_local(layout: &Layout, local: &LocalConfig) -> Result<(), MarsError> {
    let path = layout.local_config();
    save::save_document(&path, local, |text| {
        parse_local(text, &path, keys::Document::Local)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::model_policy::ModelPolicyMatchType;
    use tempfile::TempDir;

    #[test]
    fn plugin_build_rejects_unsafe_directories() {
        for (plugin_dir, expected) in [
            ("", "cannot be empty"),
            ("/tmp/plugin", "must be a relative path"),
            ("../plugin", "without `.` or `..`"),
            (".", "without `.` or `..`"),
            (".generated", "cannot use a dot directory"),
            ("generated/.private", "cannot use a dot directory"),
            ("skills/plugin", "source convention directory `skills`"),
        ] {
            let dir = TempDir::new().unwrap();
            std::fs::write(
                dir.path().join("mars.toml"),
                format!(
                    "[package]\nschema = 1\nname = \"example-plugin\"\nversion = \"1.0.0\"\n\n[plugin]\ndir = {plugin_dir:?}\n"
                ),
            )
            .unwrap();

            let config = load(&crate::layout::Layout::project(dir.path())).unwrap();
            let error = crate::build::plugin::validate_plugin_build(&config)
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains(expected),
                "dir {plugin_dir:?} should report {expected:?}, got: {error}"
            );
        }
    }

    #[test]
    fn plugin_build_validates_name_for_each_listed_harness() {
        for (harness, name, expected) in [
            ("claude", "bad:name", "Claude names"),
            ("codex", "bad--name", "Codex names"),
            ("grok", "bad/name", "Grok reads the Claude tree"),
            ("opencode", "_bad-name", "OpenCode names"),
        ] {
            let dir = TempDir::new().unwrap();
            std::fs::write(
                dir.path().join("mars.toml"),
                format!(
                    "[package]\nschema = 1\nname = \"fallback-name\"\nversion = \"1.0.0\"\n\n[plugin]\nname = {name:?}\nharnesses = [{harness:?}]\n"
                ),
            )
            .unwrap();

            let config = load(&crate::layout::Layout::project(dir.path())).unwrap();
            let error = crate::build::plugin::validate_plugin_build(&config)
                .err()
                .unwrap()
                .to_string();
            assert!(
                error.contains(expected),
                "name {name:?} for {harness} should report {expected:?}, got: {error}"
            );
        }
    }

    #[test]
    fn parse_include_filter() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
agents = ["coder"]
skills = ["review"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let (effective, _) =
            merge_with_root(config, local, &UserConfig::default(), Path::new(".")).unwrap();
        let source = &effective.dependencies["base"];
        match &source.filter {
            FilterMode::Include { agents, skills } => {
                assert_eq!(agents, &["coder"]);
                assert_eq!(skills, &["review"]);
            }
            other => panic!("expected Include, got {other:?}"),
        }
    }

    #[test]
    fn parse_exclude_filter() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
exclude = ["experimental", "deprecated"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let (effective, _) =
            merge_with_root(config, local, &UserConfig::default(), Path::new(".")).unwrap();
        let source = &effective.dependencies["base"];
        match &source.filter {
            FilterMode::Exclude(items) => {
                assert_eq!(items, &["experimental", "deprecated"]);
            }
            other => panic!("expected Exclude, got {other:?}"),
        }
    }

    #[test]
    fn error_on_both_include_and_exclude() {
        let toml_str = r#"
[dependencies.bad]
url = "https://github.com/org/bad.git"
agents = ["coder"]
exclude = ["reviewer"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("bad"),
            "error should mention dependency name: {err}"
        );
    }

    #[test]
    fn error_on_neither_url_nor_path() {
        let toml_str = r#"
[dependencies.empty]
version = "v1.0"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let err = merge_with_root(
            config,
            LocalConfig::default(),
            &UserConfig::default(),
            Path::new("."),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("neither"),
            "error should mention 'neither': {err}"
        );
    }

    #[test]
    fn error_on_both_url_and_path() {
        let toml_str = r#"
[dependencies.both]
url = "https://github.com/org/repo.git"
path = "/local/path"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let err = merge_with_root(
            config,
            LocalConfig::default(),
            &UserConfig::default(),
            Path::new("."),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("both"), "error should mention 'both': {err}");
    }

    #[test]
    fn roundtrip_full_config_shape_survives_save() {
        let dir = TempDir::new().unwrap();
        let original = r#"
[package]
schema = 1
name = "sample"
version = "0.1.0"
description = "sample package"

[dependencies.base]
url = "https://github.com/org/base.git"
version = "v1.0"
agents = ["coder", "reviewer"]

[dependencies.local]
path = "../local-agents"
exclude = ["experimental"]

[settings]
targets = [".claude", ".opencode"]
harness_order = ["pi", "opencode", "codex"]
"#;
        std::fs::write(dir.path().join("mars.toml"), original).unwrap();

        let config = load(&crate::layout::Layout::project(dir.path())).unwrap();
        save(&crate::layout::Layout::project(dir.path()), &config).unwrap();
        let reloaded = load(&crate::layout::Layout::project(dir.path())).unwrap();

        assert_eq!(
            reloaded.package.as_ref().map(|p| p.name.as_str()),
            Some("sample")
        );
        assert_eq!(reloaded.dependencies.len(), 2);
        assert_eq!(
            reloaded.dependencies["base"].url.as_deref(),
            Some("https://github.com/org/base.git")
        );
        assert_eq!(
            reloaded.dependencies["local"].path.as_deref(),
            Some(Path::new("../local-agents"))
        );
        assert_eq!(
            reloaded.settings.targets,
            Some(vec![".claude".to_string(), ".opencode".to_string()])
        );
        assert_eq!(
            reloaded.settings.harness_order,
            Some(vec![
                "pi".to_string(),
                "opencode".to_string(),
                "codex".to_string()
            ])
        );
    }

    #[test]
    fn parse_agent_overlay_and_settings_model_policies() {
        let config: Config = toml::from_str(
            r#"
[agents.tech-lead]
description = "Tech lead overlay"
model = "gpt55"
harness = "codex"
effort = "medium"
approval = "default"
sandbox = "default"
network = "allowlist"
network-allow = { replace = ["GitHub.COM.", "*.example.com"] }
network-deny = ["blocked.example"]
user-invocable = false
model-invocable = true
tools = { "Bash(rm *)" = "deny", Write = "deny" }

[[agents.tech-lead.model-policies]]
match = { alias = "gpt55" }
override = { effort = "low" }
no-fallback = true

[settings]
default-network = "deny"
network-allow = ["settings.example"]

[[settings.model-policies]]
match = { model-glob = "gpt-*" }
override = { effort = "high" }
"#,
        )
        .unwrap();

        let overlay = config.agents.get("tech-lead").expect("tech-lead overlay");
        assert_eq!(overlay.description.as_deref(), Some("Tech lead overlay"));
        assert_eq!(overlay.model.as_deref(), Some("gpt55"));
        assert_eq!(overlay.harness.as_deref(), Some("codex"));
        assert_eq!(
            overlay.network.as_ref().map(NetworkPolicy::as_str),
            Some("allowlist")
        );
        assert!(overlay.network_allow.replaces());
        assert_eq!(
            overlay
                .network_allow
                .entries()
                .iter()
                .map(NetworkHost::as_str)
                .collect::<Vec<_>>(),
            ["github.com", "*.example.com"]
        );
        assert_eq!(config.settings.default_network, Some(NetworkPolicy::Deny));
        assert_eq!(overlay.user_invocable, Some(false));
        assert_eq!(overlay.model_invocable, Some(true));
        assert_eq!(rule_keys(&overlay.tools), ["Bash(rm *)=deny", "Write=deny"]);
        let overlay_policies = overlay.model_policies.as_ref().unwrap();
        assert_eq!(overlay_policies.len(), 1);
        assert_eq!(overlay_policies[0].match_type, ModelPolicyMatchType::Alias);
        assert_eq!(overlay_policies[0].match_value, "gpt55");
        assert!(overlay_policies[0].no_fallback);

        assert_eq!(config.settings.model_policies().len(), 1);
        assert_eq!(
            config.settings.model_policies()[0].match_type,
            ModelPolicyMatchType::ModelGlob
        );
        assert_eq!(config.settings.model_policies()[0].match_value, "gpt-*");
    }

    #[test]
    fn network_overlay_rejects_typos_during_deserialization() {
        let source = "[agents.reviewer]\nnetwork = \"default\"\n";
        assert!(toml::from_str::<Config>(source).is_err());

        for source in [
            "[settings]\nnetwork-allow = [\"https://example.com\"]\n",
            "[agents.reviewer]\nnetwork-deny = [\"example.com:443\"]\n",
        ] {
            let error = toml::from_str::<Config>(source).unwrap_err().to_string();
            assert!(error.contains("invalid network host"), "{error}");
        }
    }

    fn rule_keys(map: &ToolRuleMap) -> Vec<String> {
        map.iter()
            .map(|(name, action)| format!("{}={}", String::from(name.clone()), action.as_str()))
            .collect()
    }

    #[test]
    fn tool_rules_layer_by_key_across_the_three_files() {
        let user: LocalConfig = toml::from_str(
            "[settings.tools]\nbash = \"deny\"\nread = \"deny\"\n[agents.a.tools]\n\"bash(git push *)\" = \"ask\"\n",
        )
        .unwrap();
        let config: Config = toml::from_str(
            "[settings.tools]\nshell = \"deny\"\nweb = \"deny\"\n[agents.a.tools]\n\"bash(git push:*)\" = \"allow\"\n",
        )
        .unwrap();
        let local: LocalConfig = toml::from_str(
            "[settings.tools]\n\"Web\" = \"deny\"\n[agents.a.tools]\nedit = \"deny\"\n",
        )
        .unwrap();

        let layers = compose_layers(&config, &local, &user);
        assert_eq!(
            rule_keys(&layers.settings.tools),
            ["read=deny", "shell=deny", "Web=deny"]
        );
        assert_eq!(
            rule_keys(&layers.agents["a"].tools),
            ["bash(git push:*)=allow", "edit=deny"]
        );
    }

    #[test]
    fn overlay_tools_table_reads_rules_and_refuses_a_list() {
        let file = Path::new("mars.toml");
        let config =
            parse_config("[agents.a.tools]\nweb = \"ask\"\n\"*\" = \"deny\"\n", file).unwrap();
        assert_eq!(rule_keys(&config.agents["a"].tools), ["web=ask", "*=deny"]);
        // What the user wrote saves back as written.
        let saved = toml::to_string(&config).unwrap();
        let saved = toml::from_str::<Config>(&saved).unwrap();
        assert_eq!(
            saved,
            Config {
                floor: None,
                ..config
            }
        );

        for key in ["allowed", "disallowed", "mcp"] {
            let error = parse_config(
                &format!("[agents.a.tools]\n{key} = [\"x\"]\nweb = \"ask\"\n"),
                file,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("wanted string or table"), "{error}");
        }

        for (source, expect) in [
            (
                "[agents.a.tools]\nbash = \"block\"\n",
                "unknown variant `block`",
            ),
            (
                "[agents.a.tools]\nstray = \"yes\"\n",
                "unknown variant `yes`",
            ),
            (
                "[agents.a.tools]\n\"bash(\" = \"deny\"\n",
                "invalid tool name",
            ),
            (
                "[agents.a.tools]\nBash = \"deny\"\nbash = \"ask\"\n",
                "name the same tool",
            ),
        ] {
            let error = toml::from_str::<Config>(source).unwrap_err().to_string();
            assert!(error.contains(expect), "{source}: {error}");
        }
    }

    #[test]
    fn settings_tools_take_every_action_but_the_wildcard() {
        let file = Path::new("mars.toml");
        let check = |source: &str| parse_config(source, file);
        for action in ["allow", "ask", "deny"] {
            assert!(
                check(&format!(
                    "[settings.tools]\n\"bash(git *)\" = \"{action}\"\n"
                ))
                .is_ok()
            );
            let error = check(&format!("[settings.tools]\n\"*\" = \"{action}\"\n"))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("`*` is not accepted") && error.contains("mars.toml"),
                "{error}"
            );
        }
    }

    #[test]
    fn network_model_policy_rejects_invalid_values_during_deserialization() {
        for value in ["false", "\"default\""] {
            let source = format!(
                "[settings]\n[[settings.model-policies]]\nmatch = {{ alias = \"opus\" }}\noverride = {{ network = {value} }}\n"
            );
            assert!(toml::from_str::<Config>(&source).is_err());
        }
        for key in ["network-allow", "network-deny"] {
            let source = format!(
                "[settings]\n[[settings.model-policies]]\nmatch = {{ alias = \"opus\" }}\noverride = {{ {key} = [\"example.com\"] }}\n"
            );
            assert!(toml::from_str::<Config>(&source).is_err());
        }
    }

    #[test]
    fn merged_agent_overlays_local_override_authored_fields() {
        let mut base_agents = IndexMap::new();
        base_agents.insert(
            "worker".to_string(),
            AgentOverlay {
                description: Some("Base description".to_string()),
                user_invocable: Some(true),
                ..AgentOverlay::default()
            },
        );

        let mut local_agents = IndexMap::new();
        local_agents.insert(
            "worker".to_string(),
            AgentOverlay {
                description: Some("Local description".to_string()),
                user_invocable: Some(false),
                ..AgentOverlay::default()
            },
        );
        let local = LocalConfig {
            agents: local_agents,
            ..LocalConfig::default()
        };

        let merged = overlay_agent_overlays(&base_agents, &local.agents);
        let replaced = merged.get("worker").expect("worker overlay");
        assert_eq!(replaced.description.as_deref(), Some("Local description"));
        assert_eq!(replaced.user_invocable, Some(false));
    }

    #[test]
    fn merged_agent_overlays_replace_fields_and_compose_permissions() {
        let mut base_agents = IndexMap::new();
        base_agents.insert(
            "tech-lead".to_string(),
            AgentOverlay {
                description: Some("project description".to_string()),
                model: Some("gpt55".to_string()),
                harness: Some("codex".to_string()),
                effort: Some(mars_schema::agent::EffortRequest::Level(
                    mars_schema::agent::EffortLevel::High,
                )),
                network: Some(NetworkPolicy::Deny),
                network_deny: vec!["project.example".parse().unwrap()].into(),
                tools: toml::from_str("Write = \"deny\"").unwrap(),
                model_policies: Some(vec![ModelPolicyRule {
                    match_type: ModelPolicyMatchType::Alias,
                    match_value: "project-policy".to_string(),
                    no_fallback: false,
                    overrides: Default::default(),
                    when: None,
                }]),
                ..AgentOverlay::default()
            },
        );
        base_agents.insert(
            "reviewer".to_string(),
            AgentOverlay {
                model: Some("gpt-5.4-mini".to_string()),
                ..AgentOverlay::default()
            },
        );

        let mut local_agents = IndexMap::new();
        local_agents.insert(
            "tech-lead".to_string(),
            AgentOverlay {
                model: Some("gptmini".to_string()),
                network_deny: vec!["local.example".parse().unwrap()].into(),
                tools: toml::from_str("\"Bash(git *)\" = \"allow\"\nAgent = \"deny\"").unwrap(),
                ..AgentOverlay::default()
            },
        );
        let mut local = LocalConfig {
            agents: local_agents,
            ..LocalConfig::default()
        };

        let merged = overlay_agent_overlays(&base_agents, &local.agents);
        let replaced = merged.get("tech-lead").expect("tech-lead should exist");
        assert_eq!(replaced.description, None);
        assert_eq!(replaced.model.as_deref(), Some("gptmini"));
        assert_eq!(replaced.harness, None);
        assert_eq!(replaced.effort, None);
        assert_eq!(replaced.network, None);
        assert_eq!(
            replaced
                .network_deny
                .entries()
                .iter()
                .map(NetworkHost::as_str)
                .collect::<Vec<_>>(),
            ["project.example", "local.example"]
        );
        assert_eq!(
            rule_keys(&replaced.tools),
            ["Write=deny", "Bash(git *)=allow", "Agent=deny"]
        );
        assert_eq!(replaced.model_policies, None);
        assert_eq!(
            merged
                .get("reviewer")
                .and_then(|overlay| overlay.model.as_deref()),
            Some("gpt-5.4-mini")
        );

        local.agents.get_mut("tech-lead").unwrap().model_policies = Some(Vec::new());
        assert!(
            overlay_agent_overlays(&base_agents, &local.agents)["tech-lead"]
                .model_policies
                .as_ref()
                .unwrap()
                .is_empty(),
            "an authored empty local list should replace the project list"
        );
    }

    #[test]
    fn merged_settings_applies_local_routing_overrides() {
        let settings = Settings {
            default_harness: Some("claude".to_string()),
            default_network: Some(NetworkPolicy::Deny),
            network_allow: PermissionSet::Replace {
                replace: vec!["base.example".parse().unwrap()],
            },
            network_deny: vec!["blocked.base.example".parse().unwrap()].into(),
            harness_order: Some(vec!["claude".to_string(), "pi".to_string()]),
            provider_order: Some(vec!["anthropic".to_string()]),
            ..Settings::default()
        };
        let local = LocalConfig {
            settings: LocalSettings {
                default_harness: Some("opencode".to_string()),
                default_model: Some("gpt-5".to_string()),
                default_network: Some(NetworkPolicy::Allowlist),
                network_allow: Some(vec!["local.example".parse().unwrap()].into()),
                network_deny: Some(PermissionSet::Replace {
                    replace: vec!["blocked.local.example".parse().unwrap()],
                }),
                harness_order: Some(vec!["opencode".to_string(), "pi".to_string()]),
                provider_order: Some(vec!["openai".to_string()]),
                model_policies: None,
                ..LocalSettings::default()
            },
            ..LocalConfig::default()
        };

        let merged = local.settings.overlay_settings(&settings).0;
        assert_eq!(merged.default_harness.as_deref(), Some("opencode"));
        assert_eq!(merged.default_model.as_deref(), Some("gpt-5"));
        assert_eq!(merged.default_network, Some(NetworkPolicy::Allowlist));
        assert!(merged.network_allow.replaces());
        assert_eq!(
            merged
                .network_allow
                .entries()
                .iter()
                .map(NetworkHost::as_str)
                .collect::<Vec<_>>(),
            ["base.example", "local.example"]
        );
        assert!(merged.network_deny.replaces());
        assert_eq!(
            merged
                .network_deny
                .entries()
                .iter()
                .map(NetworkHost::as_str)
                .collect::<Vec<_>>(),
            ["blocked.local.example"]
        );
        assert_eq!(
            merged.harness_order,
            Some(vec!["opencode".to_string(), "pi".to_string()])
        );
        assert_eq!(merged.provider_order, Some(vec!["openai".to_string()]));
    }

    #[test]
    fn unknown_keys_note_file_table_and_key_and_stay_in_the_file() {
        let dir = TempDir::new().unwrap();
        let project = "[futuretable]\n[settings]\nnetwork_access = 'deny'\n[agents.coder]\nmodle = 'x'\n[[agents.coder.model-policies]]\nmatch = { alias = 'a' }\nbogus = 1\n[dependencies.d]\npath = '../d'\ncolour = 'red'\n";
        std::fs::write(dir.path().join("mars.toml"), project).unwrap();
        std::fs::write(
            dir.path().join("mars.local.toml"),
            "[agents.coder]\nnetwork-denyy = ['x.test']\n",
        )
        .unwrap();

        let effective =
            load_effective_project_config(&crate::layout::Layout::project(dir.path())).unwrap();
        // The user's own `config.toml` is read too, and what it says is not this test's.
        let notes: Vec<_> = effective
            .diagnostics
            .iter()
            .filter(|note| note.code == "field.unknown" && note.message.starts_with("mars."))
            .collect();
        // The project's own files warn: a misspelt `targets` would otherwise go unseen.
        assert!(
            notes
                .iter()
                .all(|note| note.level == DiagnosticLevel::Warning && note.shown(false))
        );
        let warnings = notes
            .iter()
            .map(|note| note.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for (file, table, key) in [
            ("mars.toml", "", "futuretable"),
            ("mars.toml", "[settings] ", "network_access"),
            ("mars.toml", "[agents.coder] ", "modle"),
            ("mars.toml", "[agents.coder.model-policies[0]] ", "bogus"),
            ("mars.toml", "[dependencies.d] ", "colour"),
            ("mars.local.toml", "[agents.coder] ", "network-denyy"),
        ] {
            assert!(
                warnings.contains(&format!(
                    "{file}: {table}unknown key `{key}` is not read by Mars"
                )),
                "missing {file} {table}{key}: {warnings}"
            );
        }
        assert_eq!(notes.len(), 6, "{warnings}");

        // Saving changes only what the command changed.
        let (mut config, _) =
            load_config_with_local(&crate::layout::Layout::project(dir.path())).unwrap();
        config.settings.default_harness = Some("claude".into());
        save(&crate::layout::Layout::project(dir.path()), &config).unwrap();
        let saved = std::fs::read_to_string(dir.path().join("mars.toml")).unwrap();
        assert!(
            saved.contains("[futuretable]") && saved.contains("colour = 'red'"),
            "{saved}"
        );
    }

    #[test]
    fn underscore_spellings_load_and_gone_keys_are_unread() {
        let config = parse_config(
            "[agents.a]\nmodel_invocable = false\nnetwork_deny = ['x.test']\nautocompact = 50\n\
             [skills.s]\nuser_invocable = true\n[settings]\nnetwork_allow = ['y.test']\n\
             [settings.meridian.agent_copy]\nfanout_agents = ['a']\n",
            Path::new("mars.toml"),
        )
        .unwrap();
        assert_eq!(config.agents["a"].model_invocable, Some(false));
        assert!(!config.agents["a"].network_deny.is_noop());
        assert_eq!(config.skills["s"].user_invocable, Some(true));
        assert!(!config.settings.network_allow.is_noop());
        let unread = config
            .unknown_keys
            .iter()
            .map(|note| note.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            unread.contains("[agents.a] unknown key `autocompact`"),
            "{unread}"
        );
        assert!(unread.contains("fanout_agents"), "{unread}");
        assert_eq!(config.unknown_keys.len(), 2, "{unread}");

        let local = parse_local(
            "[settings]\nnetwork_deny = ['x.test']\n",
            Path::new("mars.local.toml"),
            keys::Document::Local,
        )
        .unwrap();
        assert!(local.unknown_keys.is_empty());
        assert!(local.settings.network_deny.is_some());
    }

    #[test]
    fn a_bad_value_is_reported_at_its_own_line() {
        for (source, line) in [
            (
                "[agents.a]\nmodel = 'x'\n[agents.a.tools]\nread = 'allow'\nwrite = 'never'\n",
                "5 | write = 'never'",
            ),
            (
                "[agents.a]\nmodel = 'x'\napproval = 'sometimes'\n",
                "3 | approval = 'sometimes'",
            ),
        ] {
            let error = parse_config(source, Path::new("mars.toml"))
                .unwrap_err()
                .to_string();
            assert!(error.contains(line), "{source}: {error}");
        }
    }

    #[test]
    fn merge_override_retains_subpath_coordinate() {
        let temp = TempDir::new().unwrap();
        // Canonicalize temp root once to avoid Windows 8.3 short-name mismatches
        let temp_root = dunce::canonicalize(temp.path()).unwrap();
        let override_path = temp_root.join("local-base");
        std::fs::create_dir_all(&override_path).unwrap();
        let canonical_override = dunce::canonicalize(&override_path).unwrap();

        let config = Config {
            dependencies: {
                let mut m = IndexMap::new();
                m.insert(
                    "base".into(),
                    mars_schema::dependency::Entry {
                        subpath: Some(SourceSubpath::new("plugins/foo").unwrap()),
                        version: Some("v1.0".into()),
                        ..mars_schema::dependency::Entry::git("https://github.com/org/base.git")
                    }
                    .into(),
                );
                m
            },
            settings: Settings::default(),
            ..Config::default()
        };
        let local = LocalConfig {
            overrides: {
                let mut m = IndexMap::new();
                m.insert(
                    "base".into(),
                    OverrideEntry {
                        path: canonical_override.clone(),
                    },
                );
                m
            },
            ..LocalConfig::default()
        };

        let (effective, _) =
            merge_with_root(config, local, &UserConfig::default(), &temp_root).unwrap();
        let source = &effective.dependencies["base"];
        assert_eq!(
            source.subpath.as_ref().map(SourceSubpath::as_str),
            Some("plugins/foo")
        );
        assert!(matches!(&source.spec, SourceSpec::Path(p) if p == &canonical_override));
        assert!(matches!(
            &source.source_id,
            SourceId::Path {
                canonical,
                subpath: Some(sp),
                plugin: None,
            } if canonical == &canonical_override && sp.as_str() == "plugins/foo"
        ));
    }

    #[test]
    fn self_dependency_name_rejected() {
        let toml_str = r#"
[dependencies._self]
url = "https://github.com/org/base.git"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("_self") && err.contains("reserved"),
            "should reject _self: {err}"
        );
    }

    #[test]
    fn save_preserves_dependencies_when_clearing_last_target() {
        let dir = TempDir::new().unwrap();
        let original = r#"
[package]
schema = 1
name = "sample"
version = "0.1.0"

[dependencies.base]
url = "https://github.com/org/base.git"
version = "v1.0"
agents = ["coder"]

[settings]
targets = [".claude"]
"#;
        std::fs::write(dir.path().join("mars.toml"), original).unwrap();

        let mut config = load(&crate::layout::Layout::project(dir.path())).unwrap();
        if let Some(targets) = config.settings.targets.as_mut() {
            targets.retain(|target| target != ".claude");
            if targets.is_empty() {
                config.settings.targets = None;
            }
        }
        save(&crate::layout::Layout::project(dir.path()), &config).unwrap();

        let reloaded = load(&crate::layout::Layout::project(dir.path())).unwrap();
        assert_eq!(
            reloaded.package.as_ref().map(|p| p.name.as_str()),
            Some("sample")
        );
        assert_eq!(
            reloaded.dependencies["base"].url.as_deref(),
            Some("https://github.com/org/base.git")
        );
        assert_eq!(
            reloaded.dependencies["base"].version.as_deref(),
            Some("v1.0")
        );
        assert_eq!(
            reloaded.dependencies["base"].filter.agents.as_deref(),
            Some(&["coder".into()][..])
        );
        assert!(reloaded.settings.targets.is_none());
    }

    #[test]
    fn roundtrip_preserves_all_filter_fields() {
        let dir = TempDir::new().unwrap();
        let original = r#"
[dependencies.include]
url = "https://github.com/org/include.git"
agents = ["coder", "reviewer"]
skills = ["review", "plan"]

[dependencies.include.rename]
coder = "core-coder"

[dependencies.exclude]
url = "https://github.com/org/exclude.git"
exclude = ["experimental", "deprecated"]

[dependencies.only_skills]
url = "https://github.com/org/skills.git"
only_skills = true

[dependencies.only_agents]
url = "https://github.com/org/agents.git"
only_agents = true
"#;
        std::fs::write(dir.path().join("mars.toml"), original).unwrap();

        let config = load(&crate::layout::Layout::project(dir.path())).unwrap();
        save(&crate::layout::Layout::project(dir.path()), &config).unwrap();
        let reloaded = load(&crate::layout::Layout::project(dir.path())).unwrap();

        let include = &reloaded.dependencies["include"].filter;
        assert_eq!(
            include.agents.as_deref(),
            Some(&["coder".into(), "reviewer".into()][..])
        );
        assert_eq!(
            include.skills.as_deref(),
            Some(&["review".into(), "plan".into()][..])
        );
        assert_eq!(
            include.rename.as_ref().and_then(|r| r.get("coder")),
            Some(&"core-coder".into())
        );

        let exclude = &reloaded.dependencies["exclude"].filter;
        assert_eq!(
            exclude.exclude.as_deref(),
            Some(&["experimental".into(), "deprecated".into()][..])
        );

        let only_skills = &reloaded.dependencies["only_skills"].filter;
        assert!(only_skills.only_skills);
        assert!(!only_skills.only_agents);

        let only_agents = &reloaded.dependencies["only_agents"].filter;
        assert!(only_agents.only_agents);
        assert!(!only_agents.only_skills);
    }

    #[test]
    fn parse_only_skills_filter() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
only_skills = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let (effective, _) =
            merge_with_root(config, local, &UserConfig::default(), Path::new(".")).unwrap();
        let source = &effective.dependencies["base"];
        assert!(matches!(source.filter, FilterMode::OnlySkills));
    }

    #[test]
    fn parse_only_agents_filter() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
only_agents = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let (effective, _) =
            merge_with_root(config, local, &UserConfig::default(), Path::new(".")).unwrap();
        let source = &effective.dependencies["base"];
        assert!(matches!(source.filter, FilterMode::OnlyAgents));
    }

    #[test]
    fn error_on_only_skills_and_only_agents() {
        let toml_str = r#"
[dependencies.bad]
url = "https://github.com/org/bad.git"
only_skills = true
only_agents = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("mutually exclusive"),
            "should mention mutually exclusive: {err}"
        );
    }

    #[test]
    fn error_on_only_skills_with_agents_list() {
        let toml_str = r#"
[dependencies.bad]
url = "https://github.com/org/bad.git"
only_skills = true
agents = ["coder"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot combine"),
            "should mention cannot combine: {err}"
        );
    }

    #[test]
    fn error_on_only_agents_with_skills_list() {
        let toml_str = r#"
[dependencies.bad]
url = "https://github.com/org/bad.git"
only_agents = true
skills = ["planning"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
    }

    #[test]
    fn error_on_only_skills_with_exclude() {
        let toml_str = r#"
[dependencies.bad]
url = "https://github.com/org/bad.git"
only_skills = true
exclude = ["deprecated"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
    }

    #[test]
    fn only_skills_true_roundtrips() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
only_skills = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.dependencies["base"].filter.only_skills);
        assert!(!config.dependencies["base"].filter.only_agents);

        let serialized = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&serialized).unwrap();
        assert!(reloaded.dependencies["base"].filter.only_skills);
    }

    // === managed_targets tests ===

    #[test]
    fn managed_targets_are_the_targets() {
        for (settings, expected) in [
            (Settings::default(), vec![]),
            (
                Settings {
                    targets: Some(vec![".claude".into()]),
                    ..Settings::default()
                },
                vec![".claude"],
            ),
        ] {
            assert_eq!(crate::config::targets::managed_targets(&settings), expected);
        }
    }

    #[test]
    fn local_targets_add_to_the_projects_and_empty_adds_nothing() {
        let project = |targets: Option<Vec<&str>>| Config {
            settings: Settings {
                targets: targets.map(|t| t.into_iter().map(String::from).collect()),
                ..Settings::default()
            },
            ..Config::default()
        };
        let local = |targets: Option<Vec<&str>>| LocalConfig {
            settings: LocalSettings {
                targets: targets.map(|t| t.into_iter().map(String::from).collect()),
                ..LocalSettings::default()
            },
            ..LocalConfig::default()
        };
        let user = LocalConfig::default();
        for (config, local_config, expected, origin) in [
            (
                vec![".claude"],
                vec![".agy", ".claude"],
                vec![".claude", ".agy"],
                targets::TargetOrigin::Local,
            ),
            (
                vec![".claude"],
                vec![],
                vec![".claude"],
                targets::TargetOrigin::Project,
            ),
            (
                vec![],
                vec![".agy"],
                vec![".agy"],
                targets::TargetOrigin::Local,
            ),
        ] {
            let layers = layering::compose_layers(
                &project((!config.is_empty()).then_some(config)),
                &local(Some(local_config)),
                &user,
            );
            assert_eq!(targets::managed_targets(&layers.settings), expected);
            assert_eq!(layers.target_source.origin, origin);
        }
    }

    // === local-dependencies tests ===

    #[test]
    fn error_on_duplicate_name_across_sections() {
        let toml_str = r#"
[dependencies.base]
url = "https://github.com/org/base.git"

[local-dependencies.base]
url = "https://github.com/org/base-local.git"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let local = LocalConfig::default();
        let result = merge_with_root(config, local, &UserConfig::default(), Path::new("."));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("base") && err.contains("both"),
            "should reject duplicate name: {err}"
        );
    }

    #[test]
    fn local_dependencies_roundtrip() {
        let dir = TempDir::new().unwrap();
        let original = r#"
[dependencies.base]
url = "https://github.com/org/base.git"

[local-dependencies.prompter]
url = "https://github.com/org/prompter.git"
skills = ["prompt-helper"]
"#;
        std::fs::write(dir.path().join("mars.toml"), original).unwrap();

        let config = load(&crate::layout::Layout::project(dir.path())).unwrap();
        save(&crate::layout::Layout::project(dir.path()), &config).unwrap();
        let reloaded = load(&crate::layout::Layout::project(dir.path())).unwrap();

        assert_eq!(reloaded.dependencies.len(), 1);
        assert_eq!(reloaded.local_dependencies.len(), 1);
        assert!(reloaded.local_dependencies.contains_key("prompter"));
        assert_eq!(
            reloaded.local_dependencies["prompter"]
                .filter
                .skills
                .as_deref(),
            Some(&["prompt-helper".into()][..])
        );
    }
}
