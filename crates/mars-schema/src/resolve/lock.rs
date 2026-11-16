use serde::{Deserialize, Serialize};

use crate::claude::manifest::PluginSource;
use crate::inspect::PluginHint;
use crate::names::SourceSubpath;

/// One `[dependencies.<name>]` row of `mars.lock`: where a dependency came from and the commit it
/// was resolved to. The serialized form is the lock file's, so a field is added only with a
/// default and never reordered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedDependency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<SourceSubpath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The marketplace plugin installed from this source. `url`, `version` and `commit` above
    /// are then the marketplace's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Where `plugin`'s files came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<LockedContent>,
}

/// The tree a marketplace plugin was read from. A plugin in the marketplace's own repository
/// repeats the marketplace's `url` and `commit` with its path as `subpath`; with a marketplace
/// at a local path, a local plugin has only its `subpath`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<SourceSubpath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// A marketplace plugin as acquired: kept with the rooted package so that preparing it again
/// (a restart pass) reads it through the same marketplace entry, never a fresh fetch.
#[derive(Debug, Clone)]
pub struct PluginRoot {
    pub hint: PluginHint,
    pub content: LockedContent,
    /// The dependency named this plugin; otherwise its marketplace lists only this one.
    pub named: bool,
}

impl PluginRoot {
    pub fn name(&self) -> &str {
        &self.hint.entry.name
    }

    /// The plugin the lock records: only one the dependency named, so a lock written for a
    /// dependency that never named one (v0.19.0's) still describes it.
    pub fn locked_name(&self) -> Option<&str> {
        self.named.then(|| self.name())
    }

    /// Where the plugin's files came from, when the lock must say: for a plugin the dependency
    /// named, or one in another repository, which a later sync fetches again by this record. An
    /// only plugin in the marketplace's own tree is the marketplace's commit and needs none.
    pub fn locked_content(&self) -> Option<&LockedContent> {
        (self.named || matches!(self.hint.entry.source, PluginSource::Remote { .. }))
            .then_some(&self.content)
    }
}
