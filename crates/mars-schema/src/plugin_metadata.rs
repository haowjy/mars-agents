//! Pure validation for Mars-generated plugin tree markers.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub const SENTINEL_FILE: &str = ".mars-plugin.json";
pub const SENTINEL_SCHEMA_VERSION: u32 = 1;

/// A package-root file Mars generates and owns.
///
/// `GENERATED_ROOT_FILES` is the only list of them: publishing excludes these
/// paths from authored-source walks, and sentinel validation accepts only them.
/// A tree row names one of these constants directly; `build::plugin::spec`'s test
/// keeps the tree rows and this list in agreement, so a tree cannot generate a
/// root file the list does not cover.
#[derive(Clone, Copy)]
pub struct GeneratedRootFile {
    path: &'static str,
    /// The catalog directory holding a marketplace file; `None` for a package
    /// manifest at the package root.
    catalog_directory: Option<&'static str>,
}

pub const CLAUDE_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: ".claude-plugin/marketplace.json",
    catalog_directory: Some(".claude-plugin"),
};
pub const CURSOR_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: ".cursor-plugin/marketplace.json",
    catalog_directory: Some(".cursor-plugin"),
};
pub const OPENCODE_ROOT_FILE: GeneratedRootFile = GeneratedRootFile {
    path: "package.json",
    catalog_directory: None,
};

pub const GENERATED_ROOT_FILES: &[GeneratedRootFile] =
    &[CLAUDE_ROOT_FILE, CURSOR_ROOT_FILE, OPENCODE_ROOT_FILE];

impl GeneratedRootFile {
    pub fn path(self) -> &'static str {
        self.path
    }

    pub fn catalog_directory(self) -> Option<&'static str> {
        self.catalog_directory
    }
}

/// Every package-root path Mars generates: the root files and their catalog directories.
pub fn generated_root_paths() -> impl Iterator<Item = &'static str> {
    GENERATED_ROOT_FILES
        .iter()
        .flat_map(|file| std::iter::once(file.path()).chain(file.catalog_directory()))
}

pub fn is_generated_root_file(path: &str) -> bool {
    GENERATED_ROOT_FILES.iter().any(|file| file.path() == path)
}

pub fn is_generated_root_directory(path: &str) -> bool {
    GENERATED_ROOT_FILES
        .iter()
        .any(|file| file.catalog_directory() == Some(path))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSentinel {
    pub schema_version: u32,
    pub mars_version: String,
    pub plugin_name: String,
    pub version: String,
    pub inputs_hash: String,
    pub dependencies: BTreeMap<String, ResolvedDependency>,
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub root_files: BTreeMap<String, String>,
    /// Root-file hashes this build replaces or deletes: the Mars-owned bytes
    /// still on disk if publication died before rewriting them. Publishing the
    /// tree and the root files takes two steps; without this record the next
    /// build could not tell its own earlier write from a hand edit. A path
    /// counts only while its bytes still match, so a user's own file at a
    /// retired path is never claimed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub root_files_previous: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub root_directories: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// Why a sentinel is not a marker for a generated plugin output directory.
///
/// Each variant names the value that failed, so a caller reports which entry is wrong
/// without repeating the rule to find it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SentinelError {
    #[error(
        "plugin sentinel schema version {recorded} is not supported (this build reads {})",
        SENTINEL_SCHEMA_VERSION
    )]
    UnsupportedSchemaVersion { recorded: u32 },
    #[error("plugin sentinel records an empty `{field}`")]
    EmptyField { field: &'static str },
    #[error("plugin sentinel dependency `{name}` records neither a version nor a commit")]
    DependencyWithoutVersionOrCommit { name: String },
    #[error("plugin sentinel records no generated files")]
    NoFiles,
    #[error("plugin sentinel entry `{path}` is not a relative path with a sha256 hash")]
    InvalidEntry { path: String },
    #[error("plugin sentinel records unsupported generated root file `{path}`")]
    UnsupportedRootFile { path: String },
    #[error("plugin sentinel records unsupported generated root directory `{path}`")]
    UnsupportedRootDirectory { path: String },
}

impl PluginSentinel {
    /// Why this sentinel does not describe a tree this build may treat as generated output.
    pub fn validate(&self) -> Result<(), SentinelError> {
        if self.schema_version != SENTINEL_SCHEMA_VERSION {
            return Err(SentinelError::UnsupportedSchemaVersion {
                recorded: self.schema_version,
            });
        }
        let required = [
            ("mars_version", &self.mars_version),
            ("plugin_name", &self.plugin_name),
            ("version", &self.version),
            ("inputs_hash", &self.inputs_hash),
        ];
        if let Some((field, _)) = required
            .into_iter()
            .find(|(_, value)| value.trim().is_empty())
        {
            return Err(SentinelError::EmptyField { field });
        }
        for (name, dependency) in &self.dependencies {
            if !records_revision(&dependency.version) && !records_revision(&dependency.commit) {
                return Err(SentinelError::DependencyWithoutVersionOrCommit { name: name.clone() });
            }
        }
        if self.files.is_empty() {
            return Err(SentinelError::NoFiles);
        }
        for (path, hash) in &self.files {
            validate_entry(path, hash)?;
        }
        for (path, hash) in self.root_files.iter().chain(&self.root_files_previous) {
            if !is_generated_root_file(path) {
                return Err(SentinelError::UnsupportedRootFile { path: path.clone() });
            }
            validate_entry(path, hash)?;
        }
        if let Some(path) = self
            .root_directories
            .iter()
            .find(|path| !is_generated_root_directory(path))
        {
            return Err(SentinelError::UnsupportedRootDirectory { path: path.clone() });
        }
        Ok(())
    }

    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }
}

/// Whether bytes are a valid marker for a generated plugin output directory.
pub fn is_valid_sentinel(bytes: &[u8]) -> bool {
    serde_json::from_slice::<PluginSentinel>(bytes).is_ok_and(|sentinel| sentinel.is_valid())
}

fn validate_entry(path: &str, hash: &str) -> Result<(), SentinelError> {
    if valid_entry(path, hash) {
        return Ok(());
    }
    Err(SentinelError::InvalidEntry {
        path: path.to_string(),
    })
}

/// Whether a dependency pins the build to something: a version, a commit, or both.
fn records_revision(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Whether a sentinel entry names a package-relative path and a sha256 hash.
pub fn valid_entry(path: &str, hash: &str) -> bool {
    !path.is_empty()
        && !is_absolute_on_any_platform(path)
        && !path.split(['/', '\\']).any(|component| component == "..")
        && is_sha256(hash)
}

/// Reject roots of every target platform, not only the host's: a sentinel written
/// on Windows must not read as relative when checked on Unix, or the reverse.
fn is_absolute_on_any_platform(path: &str) -> bool {
    path.starts_with(['/', '\\'])
        || path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && path.as_bytes().get(1) == Some(&b':')
}

pub fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
