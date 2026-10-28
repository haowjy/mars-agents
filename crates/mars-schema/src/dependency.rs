//! One `[dependencies]` or `[local-dependencies]` entry, as a package manifest or a consumer
//! `mars.toml` writes it: its source, selectors and filter, and the rules that hold wherever it is
//! read. The consumer's configuration and a package's manifest reading both parse through
//! [`Entry::from_raw`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::diagnostic::{SchemaDiagnostic, Severity, child_path};
use crate::dialect::Dialect;
use crate::filter::FilterConfig;
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
/// it; [`Entry::from_raw`] applies the rules where an entry is used.
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
}

impl EntryError {
    /// The diagnostic code the manifest reading reports it under.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BothSources { .. } | Self::NoSource { .. } => "dependency.source",
            Self::PluginSelector(_) => "dependency.plugin-selector",
        }
    }
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

    /// Apply the rules every reader shares: exactly one of `url` and `path`, and a `plugin`
    /// selector that does not contradict `subpath` or `dialect`. The filter's own consistency is
    /// [`FilterConfig::validate`], which the caller applies where it refuses a conflict.
    pub fn from_raw(name: &str, raw: RawEntry) -> Result<Self, EntryError> {
        let source = match (raw.url, raw.path) {
            (Some(url), None) => Source::Git { url },
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
        Ok(Self {
            source,
            version: raw.version,
            subpath: raw.subpath,
            dialect: raw.dialect,
            plugin: raw.plugin,
            filter: raw.filter,
            unknown: raw.unknown,
        })
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
        let mut raw = match raw {
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
        if raw.plugin.is_some() && raw.dialect.take().is_some() {
            diagnostics.push(SchemaDiagnostic::new(
                "dependency.dialect-ignored",
                Severity::Warning,
                path.clone(),
                format!("source `{name}` has both `plugin` and `dialect`; the dialect is ignored"),
            ));
        }
        match Entry::from_raw(name, raw) {
            Ok(entry) => {
                // A package's entry is read as written, and its dependents' syncs must not
                // break on it: a conflicting filter warns, and `FilterConfig::to_mode` picks the
                // mode it always has.
                if let Err(conflict) = entry.filter.validate(name) {
                    diagnostics.push(SchemaDiagnostic::new(
                        "dependency.filter-conflict",
                        Severity::Warning,
                        path,
                        conflict.to_string(),
                    ));
                }
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
