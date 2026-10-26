//! Shared metadata for Mars-owned generated plugin output directories.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::MarsError;

pub(crate) const SENTINEL_FILE: &str = ".mars-plugin.json";
pub(crate) const SENTINEL_SCHEMA_VERSION: u32 = 1;

// Generated root files and their catalog directories live here, below
// `build::plugin::spec`'s format rows, because staging and sentinel validation
// only need the paths. The tree rows reference these constants, so a path is
// declared once.
pub(crate) const CLAUDE_MARKETPLACE_FILE: &str = ".claude-plugin/marketplace.json";
pub(crate) const CLAUDE_CATALOG_DIRECTORY: &str = ".claude-plugin";
pub(crate) const CURSOR_MARKETPLACE_FILE: &str = ".cursor-plugin/marketplace.json";
pub(crate) const CURSOR_CATALOG_DIRECTORY: &str = ".cursor-plugin";
pub(crate) const OPENCODE_PACKAGE_FILE: &str = "package.json";

pub(crate) const GENERATED_ROOT_FILES: [&str; 3] = [
    CLAUDE_MARKETPLACE_FILE,
    CURSOR_MARKETPLACE_FILE,
    OPENCODE_PACKAGE_FILE,
];

pub(crate) const GENERATED_ROOT_DIRECTORIES: [&str; 2] =
    [CLAUDE_CATALOG_DIRECTORY, CURSOR_CATALOG_DIRECTORY];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PluginSentinel {
    pub schema_version: u32,
    pub mars_version: String,
    pub plugin_name: String,
    pub version: String,
    pub inputs_hash: String,
    pub dependencies: std::collections::BTreeMap<String, ResolvedDependency>,
    pub files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub root_files: std::collections::BTreeMap<String, String>,
    /// Root-file hashes this build replaces or deletes: the Mars-owned bytes
    /// still on disk if publication died before rewriting them. Publishing the
    /// tree and the root files takes two steps; without this record the next
    /// build could not tell its own earlier write from a hand edit. A path
    /// counts only while its bytes still match, so a user's own file at a
    /// retired path is never claimed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub root_files_previous: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub root_directories: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

impl PluginSentinel {
    fn is_valid(&self) -> bool {
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
            && self
                .files
                .iter()
                .all(|(path, hash)| valid_entry(path, hash))
            && self
                .root_files
                .iter()
                .all(|(path, hash)| valid_entry(path, hash))
            && self
                .root_files_previous
                .iter()
                .all(|(path, hash)| valid_entry(path, hash))
            && self
                .root_directories
                .iter()
                .all(|path| GENERATED_ROOT_DIRECTORIES.contains(&path.as_str()))
    }
}

fn valid_entry(path: &str, hash: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && !path.split('/').any(|component| component == "..")
        && is_sha256(hash)
}

pub(crate) fn validate(sentinel: &PluginSentinel, path: &Path) -> Result<(), MarsError> {
    if let Some(unsupported) = sentinel
        .root_files
        .keys()
        .chain(sentinel.root_files_previous.keys())
        .find(|recorded| !GENERATED_ROOT_FILES.contains(&recorded.as_str()))
    {
        return Err(invalid(format!(
            "plugin sentinel records unsupported generated root file `{unsupported}`: {}",
            path.display()
        )));
    }
    if let Some(unsupported) = sentinel
        .root_directories
        .iter()
        .find(|recorded| !GENERATED_ROOT_DIRECTORIES.contains(&recorded.as_str()))
    {
        return Err(invalid(format!(
            "plugin sentinel records unsupported generated root directory `{unsupported}`: {}",
            path.display()
        )));
    }
    if sentinel.is_valid() {
        Ok(())
    } else {
        Err(invalid(format!(
            "plugin sentinel has invalid metadata or unsupported schema {}: {}",
            sentinel.schema_version,
            path.display()
        )))
    }
}

/// Read and validate a generated plugin output directory's root sentinel.
pub(crate) fn read(directory: &Path) -> Result<PluginSentinel, MarsError> {
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|source| io_error("inspect plugin tree", directory, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(format!(
            "plugin tree must be a directory, not {}",
            directory.display()
        )));
    }

    let path = directory.join(SENTINEL_FILE);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|source| io_error("inspect plugin sentinel", &path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid(format!(
            "plugin sentinel must be a regular file: {}",
            path.display()
        )));
    }
    let bytes =
        std::fs::read(&path).map_err(|source| io_error("read plugin sentinel", &path, source))?;
    let sentinel: PluginSentinel = serde_json::from_slice(&bytes).map_err(|error| {
        invalid(format!(
            "invalid plugin sentinel {}: {error}",
            path.display()
        ))
    })?;
    validate(&sentinel, &path)?;
    Ok(sentinel)
}

/// Whether `directory` is generated plugin output that authored-source walks
/// must not treat as package input. Harness trees live below this boundary.
pub(crate) fn has_valid_plugin_sentinel(directory: &Path) -> bool {
    read(directory).is_ok()
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn invalid(message: impl Into<String>) -> MarsError {
    MarsError::InvalidRequest {
        message: message.into(),
    }
}

fn io_error(operation: &str, path: &Path, source: std::io::Error) -> MarsError {
    MarsError::Io {
        operation: operation.to_string(),
        path: path.to_path_buf(),
        source,
    }
}
