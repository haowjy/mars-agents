//! One `[dependencies]` or `[local-dependencies]` entry, as a package manifest or a consumer
//! `mars.toml` writes it: its source, selectors and filter, and the rules that hold wherever it is
//! read. The consumer's configuration, a package's manifest reading and `mars add` all read an
//! entry through [`Entry::read`], which holds every rule and says, by [`Strictness`], which of them
//! refuse and which only warn.

use std::collections::BTreeMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::diagnostic::{SchemaDiagnostic, Severity, child_path};
use crate::dialect::Dialect;
use crate::filter::{FilterConfig, FilterError};
use crate::names::SourceSubpath;

/// Where an entry's package comes from: a git repository or a directory, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Git { url: String },
    Path { path: PathBuf },
}

/// One dependency entry. It serializes in the spelling of the file (`url` or `path`, `version`,
/// `subpath`, `dialect`, `plugin`, the filter keys), so a consumer's `mars.toml` writes it back as
/// it was authored. `version` is a git source's constraint; a path source has none and ignores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub source: Source,
    pub version: Option<String>,
    pub subpath: Option<SourceSubpath>,
    /// Inbound lift dialect for the package.
    pub dialect: Option<Dialect>,
    /// The plugin to install from the marketplace at this source.
    pub plugin: Option<String>,
    pub filter: FilterConfig,
    /// Keys Mars does not read here, kept so the file round-trips and a caller can warn.
    pub unknown: BTreeMap<String, toml::Value>,
}

/// The authored shape of an entry, before the rules that need its name. A consumer's `mars.toml`
/// holds entries in this shape, so a file with a bad entry still loads and `mars remove` can repair
/// it; [`Entry::read`] applies the rules where an entry is used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "path_serde")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<SourceSubpath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<Dialect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(flatten)]
    pub filter: FilterConfig,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// A path is written with `/` on every host.
mod path_serde {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match path {
            Some(path) => serializer.serialize_some(&path.to_string_lossy().replace('\\', "/")),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map(PathBuf::from))
    }
}

/// An entry the schema refuses, by the rule it breaks. Each message names the entry.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EntryError {
    #[error("source `{name}` has both `url` and `path` — pick one")]
    BothSources { name: String },
    #[error("source `{name}` has neither `url` nor `path` — one is required")]
    NoSource { name: String },
    #[error(transparent)]
    PluginSelector(#[from] PluginSelectorError),
    /// A filter that contradicts itself, refused under [`Strictness::Consumer`].
    #[error(transparent)]
    Filter(#[from] FilterError),
}

impl EntryError {
    /// The diagnostic code the manifest reading reports it under.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BothSources { .. } | Self::NoSource { .. } => "dependency.source",
            Self::PluginSelector(_) => "dependency.plugin-selector",
            Self::Filter(_) => "dependency.filter-conflict",
        }
    }
}

/// Who an entry is read for, which decides what a rule does when it breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// The project's own `mars.toml`, or an entry about to be written to it: every rule refuses.
    Consumer,
    /// A dependency's manifest, read as written: its dependents' syncs must not break on it, so a
    /// conflicting filter warns (and `FilterConfig::to_mode` picks the mode it always has) and a
    /// `plugin` entry's `dialect` is dropped with a warning.
    Package,
}

/// A rule a [`Strictness::Package`] entry broke and was kept anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryWarning {
    pub code: &'static str,
    pub message: String,
}

impl Entry {
    /// A git entry with no selector, version or filter.
    pub fn git(url: impl Into<String>) -> Self {
        Self::from_source(Source::Git { url: url.into() })
    }

    /// A path entry with no selector or filter.
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self::from_source(Source::Path { path: path.into() })
    }

    /// An entry for `source` with no selector, version or filter.
    pub fn from_source(source: Source) -> Self {
        Self {
            source,
            version: None,
            subpath: None,
            dialect: None,
            plugin: None,
            filter: FilterConfig::default(),
            unknown: BTreeMap::new(),
        }
    }

    /// The git URL, for a git entry.
    pub fn url(&self) -> Option<&str> {
        match &self.source {
            Source::Git { url } => Some(url),
            Source::Path { .. } => None,
        }
    }

    /// The directory, for a path entry.
    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.source {
            Source::Path { path } => Some(path),
            Source::Git { .. } => None,
        }
    }

    /// Read an authored entry under every rule: exactly one of `url` and `path`, a bare
    /// `host/path` URL spelled with `https://` (see [`crate::git_url::with_default_scheme`]), a
    /// `plugin` selector that does not contradict `subpath` or `dialect`, and a consistent filter
    /// ([`FilterConfig::validate`]). `strictness` decides whether a broken filter or a `plugin`
    /// with `dialect` refuses or warns; the rest refuse for every reader.
    pub fn read(
        name: &str,
        mut raw: RawEntry,
        strictness: Strictness,
    ) -> Result<(Self, Vec<EntryWarning>), EntryError> {
        let mut warnings = Vec::new();
        if strictness == Strictness::Package && raw.plugin.is_some() && raw.dialect.take().is_some()
        {
            warnings.push(EntryWarning {
                code: "dependency.dialect-ignored",
                message: format!(
                    "source `{name}` has both `plugin` and `dialect`; the dialect is ignored"
                ),
            });
        }
        let source = match (raw.url, raw.path) {
            (Some(url), None) => Source::Git {
                url: crate::git_url::with_default_scheme(&url),
            },
            (None, Some(path)) => Source::Path { path },
            (Some(_), Some(_)) => return Err(EntryError::BothSources { name: name.into() }),
            (None, None) => return Err(EntryError::NoSource { name: name.into() }),
        };
        validate_plugin_selector(
            name,
            raw.plugin.as_deref(),
            raw.subpath.as_ref(),
            raw.dialect,
        )?;
        match (raw.filter.validate(name), strictness) {
            (Err(conflict), Strictness::Consumer) => return Err(conflict.into()),
            (Err(conflict), Strictness::Package) => warnings.push(EntryWarning {
                code: "dependency.filter-conflict",
                message: conflict.to_string(),
            }),
            (Ok(()), _) => {}
        }
        let entry = Self {
            source,
            version: raw.version,
            subpath: raw.subpath,
            dialect: raw.dialect,
            plugin: raw.plugin,
            filter: raw.filter,
            unknown: raw.unknown,
        };
        Ok((entry, warnings))
    }
}

impl From<&Entry> for RawEntry {
    fn from(entry: &Entry) -> Self {
        let (url, path) = match &entry.source {
            Source::Git { url } => (Some(url.clone()), None),
            Source::Path { path } => (None, Some(path.clone())),
        };
        Self {
            url,
            path,
            subpath: entry.subpath.clone(),
            version: entry.version.clone(),
            dialect: entry.dialect,
            plugin: entry.plugin.clone(),
            filter: entry.filter.clone(),
            unknown: entry.unknown.clone(),
        }
    }
}

impl From<Entry> for RawEntry {
    fn from(entry: Entry) -> Self {
        Self::from(&entry)
    }
}

impl Serialize for Entry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RawEntry::from(self).serialize(serializer)
    }
}

/// A package manifest's two dependency tables, entries by name in declaration order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Dependencies {
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub dependencies: IndexMap<String, Entry>,
    #[serde(
        rename = "local-dependencies",
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub local_dependencies: IndexMap<String, Entry>,
}

impl Dependencies {
    /// Read both tables of a parsed `mars.toml`, entries in declaration order. An entry that
    /// breaks a rule is left out and reported at `[<table>.<name>]`. A refusal in `[dependencies]`
    /// is an error, because a dependent installs from it; `[local-dependencies]` is never
    /// installed from by a dependent, so its refusals are warnings. A filter conflict, and a
    /// `plugin` entry's `dialect` (ignored there), are warnings on an entry that is kept. A table
    /// that is not a table leaves both out with one error.
    pub fn read(
        document: &toml_edit::DocumentMut,
        diagnostics: &mut Vec<SchemaDiagnostic>,
    ) -> Self {
        let tables = ["dependencies", "local-dependencies"].map(|key| match document.get(key) {
            None => Ok(None),
            Some(item) => item.as_table_like().map(Some).ok_or(key),
        });
        let [Ok(dependencies), Ok(local_dependencies)] = tables else {
            let bad = tables.iter().find_map(|table| table.as_ref().err());
            diagnostics.push(SchemaDiagnostic::new(
                "dependency.invalid",
                Severity::Error,
                Vec::new(),
                format!(
                    "`dependencies` and `local-dependencies` must be tables: `{}` is not",
                    bad.copied().unwrap_or_default()
                ),
            ));
            return Self::default();
        };
        Self {
            dependencies: read_table("dependencies", dependencies, Severity::Error, diagnostics),
            local_dependencies: read_table(
                "local-dependencies",
                local_dependencies,
                Severity::Warning,
                diagnostics,
            ),
        }
    }
}

/// Read one table's entries. A refused entry is left out and reported at `refusal` severity.
fn read_table(
    table: &str,
    items: Option<&dyn toml_edit::TableLike>,
    refusal: Severity,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> IndexMap<String, Entry> {
    let mut entries = IndexMap::new();
    for (name, item) in items.into_iter().flat_map(|items| items.iter()) {
        let path = child_path(table, name);
        let key_path = format!("{table}.{name}");
        let raw = match item.clone().into_value() {
            Ok(value) => RawEntry::deserialize(
                serde::de::IntoDeserializer::<toml_edit::de::Error>::into_deserializer(value),
            )
            .map_err(|error| format!("{key_path}: {}", error.to_string().trim_end())),
            Err(_) => Err(format!("{key_path}: must be a table")),
        };
        let raw = match raw {
            Ok(raw) => raw,
            Err(message) => {
                diagnostics.push(SchemaDiagnostic::new(
                    "dependency.invalid",
                    refusal,
                    path,
                    message,
                ));
                continue;
            }
        };
        match Entry::read(name, raw, Strictness::Package) {
            Ok((entry, warnings)) => {
                diagnostics.extend(warnings.into_iter().map(|warning| {
                    SchemaDiagnostic::new(
                        warning.code,
                        Severity::Warning,
                        path.clone(),
                        warning.message,
                    )
                }));
                entries.insert(name.to_owned(), entry);
            }
            Err(error) => diagnostics.push(SchemaDiagnostic::new(
                error.code(),
                refusal,
                path,
                error.to_string(),
            )),
        }
    }
    entries
}

/// A dependency entry that selects a marketplace plugin with a key that contradicts it.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PluginSelectorError {
    #[error("source `{name}` has `plugin = \"{plugin}\"`; a plugin name is {rule}")]
    BadName {
        name: String,
        plugin: String,
        rule: &'static str,
    },
    #[error(
        "source `{name}` has both `plugin` and `subpath` — a marketplace is read at the root of its source, and the marketplace entry says where the plugin is"
    )]
    WithSubpath { name: String },
    #[error(
        "source `{name}` has both `plugin` and `dialect` — a marketplace plugin is read as a Claude plugin"
    )]
    WithDialect { name: String },
}

/// `plugin` picks one plugin of the marketplace at the source's root, read as a Claude
/// marketplace; a `subpath` or `dialect` would name a different root or reading.
pub fn validate_plugin_selector(
    name: &str,
    plugin: Option<&str>,
    subpath: Option<&SourceSubpath>,
    dialect: Option<Dialect>,
) -> Result<(), PluginSelectorError> {
    let Some(plugin) = plugin else {
        return Ok(());
    };
    let name = name.to_string();
    if !crate::claude::manifest::is_plugin_name(plugin) {
        return Err(PluginSelectorError::BadName {
            name,
            plugin: plugin.to_string(),
            rule: crate::claude::manifest::PLUGIN_NAME_RULE,
        });
    }
    if subpath.is_some() {
        return Err(PluginSelectorError::WithSubpath { name });
    }
    if dialect.is_some() {
        return Err(PluginSelectorError::WithDialect { name });
    }
    Ok(())
}
