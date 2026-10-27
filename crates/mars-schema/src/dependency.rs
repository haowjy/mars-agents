//! One `[dependencies]` or `[local-dependencies]` entry, as a package manifest or a consumer
//! `mars.toml` writes it: its source, selectors and filter, and the rules that hold wherever it is
//! read. The consumer's configuration and a package's manifest reading both parse through
//! [`Entry::from_raw`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

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

/// The authored shape of an entry, before the rules that need its name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "path_serde")]
    path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subpath: Option<SourceSubpath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dialect: Option<Dialect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<String>,
    #[serde(flatten)]
    filter: FilterConfig,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    unknown: BTreeMap<String, toml::Value>,
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

impl Serialize for Entry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RawEntry::from(self).serialize(serializer)
    }
}

/// One `[dependencies]` or `[local-dependencies]` table: entries by name, in declaration order.
/// Deserializing refuses the first entry [`Entry::from_raw`] refuses.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Entries<K>(IndexMap<K, Entry>);

impl<K: std::hash::Hash + Eq> PartialEq for Entries<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K> Entries<K> {
    pub fn into_map(self) -> IndexMap<K, Entry> {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K: std::hash::Hash + Eq> Default for Entries<K> {
    fn default() -> Self {
        Self(IndexMap::new())
    }
}

impl<K: std::hash::Hash + Eq> std::ops::Deref for Entries<K> {
    type Target = IndexMap<K, Entry>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K: std::hash::Hash + Eq> std::ops::DerefMut for Entries<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, K> IntoIterator for &'a Entries<K> {
    type Item = (&'a K, &'a Entry);
    type IntoIter = indexmap::map::Iter<'a, K, Entry>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<K> IntoIterator for Entries<K> {
    type Item = (K, Entry);
    type IntoIter = indexmap::map::IntoIter<K, Entry>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<K: std::hash::Hash + Eq> FromIterator<(K, Entry)> for Entries<K> {
    fn from_iter<I: IntoIterator<Item = (K, Entry)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'de, K> Deserialize<'de> for Entries<K>
where
    K: Deserialize<'de> + AsRef<str> + std::hash::Hash + Eq,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = IndexMap::<K, RawEntry>::deserialize(deserializer)?;
        raw.into_iter()
            .map(|(name, raw)| {
                let entry =
                    Entry::from_raw(name.as_ref(), raw).map_err(serde::de::Error::custom)?;
                Ok((name, entry))
            })
            .collect()
    }
}

/// A package manifest's two dependency tables.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Dependencies {
    #[serde(skip_serializing_if = "Entries::is_empty")]
    pub dependencies: Entries<String>,
    #[serde(
        rename = "local-dependencies",
        skip_serializing_if = "Entries::is_empty"
    )]
    pub local_dependencies: Entries<String>,
}

impl Dependencies {
    /// Read both tables of a `mars.toml` that parses as TOML, entries in declaration order. An
    /// entry that breaks a rule is left out and reported as an error at `[<table>.<name>]`, and a
    /// filter conflict is a warning on an entry that is kept; a table that is not a table leaves both out with one error.
    pub fn read(content: &str, diagnostics: &mut Vec<SchemaDiagnostic>) -> Self {
        #[derive(Deserialize)]
        struct Tables {
            #[serde(default)]
            dependencies: IndexMap<String, toml::Value>,
            #[serde(default, rename = "local-dependencies")]
            local_dependencies: IndexMap<String, toml::Value>,
        }
        let tables = match toml::from_str::<Tables>(content) {
            Ok(tables) => tables,
            Err(error) => {
                diagnostics.push(SchemaDiagnostic::new(
                    "dependency.invalid",
                    Severity::Error,
                    Vec::new(),
                    format!("`dependencies` and `local-dependencies` must be tables: {error}"),
                ));
                return Self::default();
            }
        };
        Self {
            dependencies: read_table("dependencies", tables.dependencies, diagnostics),
            local_dependencies: read_table(
                "local-dependencies",
                tables.local_dependencies,
                diagnostics,
            ),
        }
    }
}

fn read_table(
    table: &str,
    values: IndexMap<String, toml::Value>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Entries<String> {
    let mut entries = Entries::default();
    for (name, value) in values {
        let path = child_path(table, &name);
        let parsed = value
            .try_into::<RawEntry>()
            .map_err(|error| ("dependency.invalid", error.to_string()))
            .and_then(|raw| {
                Entry::from_raw(&name, raw).map_err(|error| (error.code(), error.to_string()))
            });
        match parsed {
            Ok(entry) => {
                // A package's entry is read as written, and its dependents' syncs must not
                // break on it: a conflicting filter warns, and `FilterConfig::to_mode` picks the
                // mode it always has.
                if let Err(conflict) = entry.filter.validate(&name) {
                    diagnostics.push(SchemaDiagnostic::new(
                        "dependency.filter-conflict",
                        Severity::Warning,
                        path,
                        conflict.to_string(),
                    ));
                }
                entries.insert(name, entry);
            }
            Err((code, message)) => {
                diagnostics.push(SchemaDiagnostic::new(code, Severity::Error, path, message))
            }
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
