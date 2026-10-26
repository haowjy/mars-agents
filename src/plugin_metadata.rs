//! Shared metadata for Mars-owned generated plugin output directories.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::MarsError;

pub(crate) const SENTINEL_FILE: &str = ".mars-plugin.json";
pub(crate) const SENTINEL_SCHEMA_VERSION: u32 = 1;

// Generated root files live here, below `build::plugin`'s tree rows, because
// sentinel validation and staging read them from a lower layer.

/// A package-root file Mars generates and owns.
///
/// `GENERATED_ROOT_FILES` is the only list of them: publishing excludes these
/// paths from authored-source walks, and sentinel validation accepts only them.
/// A tree row takes its file from [`Self::marketplace`] or [`Self::package`],
/// which fail to compile unless the entry is declared, so a tree cannot generate
/// a root file the list does not cover.
#[derive(Clone, Copy)]
pub(crate) struct GeneratedRootFile {
    path: &'static str,
    /// The catalog directory holding a marketplace file; `None` for a package
    /// manifest at the package root.
    catalog_directory: Option<&'static str>,
}

pub(crate) const CLAUDE_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: ".claude-plugin/marketplace.json",
    catalog_directory: Some(".claude-plugin"),
};
pub(crate) const CURSOR_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: ".cursor-plugin/marketplace.json",
    catalog_directory: Some(".cursor-plugin"),
};
pub(crate) const OPENCODE_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: "package.json",
    catalog_directory: None,
};

pub(crate) const GENERATED_ROOT_FILES: &[GeneratedRootFile] =
    &[CLAUDE_ROOT_FILE, CURSOR_ROOT_FILE, OPENCODE_ROOT_FILE];

impl GeneratedRootFile {
    pub(crate) fn path(self) -> &'static str {
        self.path
    }

    pub(crate) fn catalog_directory(self) -> Option<&'static str> {
        self.catalog_directory
    }

    /// The declared marketplace catalog `file` names.
    pub(crate) const fn marketplace(file: Self) -> Self {
        Self::declared(file, true)
    }

    /// The declared package manifest file `file` names.
    pub(crate) const fn package(file: Self) -> Self {
        Self::declared(file, false)
    }

    const fn declared(file: Self, has_catalog_directory: bool) -> Self {
        let mut index = 0;
        while index < GENERATED_ROOT_FILES.len() {
            let declared = GENERATED_ROOT_FILES[index];
            if str_eq(declared.path, file.path) {
                assert!(declared.catalog_directory.is_some() == has_catalog_directory);
                return declared;
            }
            index += 1;
        }
        panic!("generated root file is not declared in GENERATED_ROOT_FILES");
    }
}

const fn str_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Every package-root path Mars generates: the root files and their catalog directories.
pub(crate) fn generated_root_paths() -> impl Iterator<Item = &'static str> {
    GENERATED_ROOT_FILES
        .iter()
        .flat_map(|file| std::iter::once(file.path).chain(file.catalog_directory))
}

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
                .all(|path| is_generated_root_directory(path))
    }
}

fn is_generated_root_file(path: &str) -> bool {
    GENERATED_ROOT_FILES.iter().any(|file| file.path == path)
}

fn is_generated_root_directory(path: &str) -> bool {
    GENERATED_ROOT_FILES
        .iter()
        .any(|file| file.catalog_directory == Some(path))
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
        .find(|recorded| !is_generated_root_file(recorded))
    {
        return Err(invalid(format!(
            "plugin sentinel records unsupported generated root file `{unsupported}`: {}",
            path.display()
        )));
    }
    if let Some(unsupported) = sentinel
        .root_directories
        .iter()
        .find(|recorded| !is_generated_root_directory(recorded))
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
