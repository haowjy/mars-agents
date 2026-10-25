//! Shared metadata for Mars-owned generated plugin trees.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub(crate) const SENTINEL_FILE: &str = ".mars-plugin.json";
pub(crate) const SENTINEL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PluginSentinel {
    pub schema_version: u32,
    pub mars_version: String,
    pub plugin_name: String,
    pub version: String,
    pub inputs_hash: String,
    pub dependencies: std::collections::BTreeMap<String, ResolvedDependency>,
    pub files: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

impl PluginSentinel {
    pub(crate) fn is_valid(&self) -> bool {
        self.schema_version == SENTINEL_SCHEMA_VERSION
            && !self.mars_version.trim().is_empty()
            && !self.plugin_name.trim().is_empty()
            && !self.version.trim().is_empty()
            && !self.inputs_hash.trim().is_empty()
            && self.dependencies.values().all(|dependency| {
                dependency
                    .version
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                    || dependency
                        .commit
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty())
            })
            && !self.files.is_empty()
            && self.files.iter().all(|(path, hash)| {
                !path.is_empty()
                    && !Path::new(path).is_absolute()
                    && !path.split('/').any(|component| component == "..")
                    && is_sha256(hash)
            })
    }
}

/// Whether `directory` is a generated plugin tree that discovery and flat-skill
/// projection must not treat as authored package input.
pub(crate) fn has_valid_plugin_sentinel(directory: &Path) -> bool {
    let path = directory.join(SENTINEL_FILE);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PluginSentinel>(&bytes).ok())
        .is_some_and(|sentinel| sentinel.is_valid())
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
