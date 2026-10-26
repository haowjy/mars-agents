//! Pure validation for Mars-generated plugin tree markers.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

pub const SENTINEL_FILE: &str = ".mars-plugin.json";
const SENTINEL_SCHEMA_VERSION: u32 = 1;
const GENERATED_ROOT_FILES: &[&str] = &[
    ".claude-plugin/marketplace.json",
    ".cursor-plugin/marketplace.json",
    "package.json",
];
const GENERATED_ROOT_DIRECTORIES: &[&str] = &[".claude-plugin", ".cursor-plugin"];

#[derive(Deserialize)]
struct PluginSentinel {
    schema_version: u32,
    mars_version: String,
    plugin_name: String,
    version: String,
    inputs_hash: String,
    dependencies: BTreeMap<String, ResolvedDependency>,
    files: BTreeMap<String, String>,
    #[serde(default)]
    root_files: BTreeMap<String, String>,
    #[serde(default)]
    root_directories: BTreeSet<String>,
}

#[derive(Deserialize)]
struct ResolvedDependency {
    version: Option<String>,
    commit: Option<String>,
}

/// Whether bytes are a valid marker for a generated plugin output directory.
pub fn is_valid_sentinel(bytes: &[u8]) -> bool {
    let Ok(sentinel) = serde_json::from_slice::<PluginSentinel>(bytes) else {
        return false;
    };
    sentinel.schema_version == SENTINEL_SCHEMA_VERSION
        && !sentinel.mars_version.trim().is_empty()
        && !sentinel.plugin_name.trim().is_empty()
        && !sentinel.version.trim().is_empty()
        && !sentinel.inputs_hash.trim().is_empty()
        && sentinel.dependencies.values().all(|dependency| {
            dependency
                .version
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || dependency
                    .commit
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        })
        && !sentinel.files.is_empty()
        && sentinel
            .files
            .iter()
            .all(|(path, hash)| valid_entry(path, hash))
        && sentinel
            .root_files
            .iter()
            .all(|(path, hash)| GENERATED_ROOT_FILES.contains(&path.as_str()) && is_sha256(hash))
        && sentinel
            .root_directories
            .iter()
            .all(|path| GENERATED_ROOT_DIRECTORIES.contains(&path.as_str()))
}

fn valid_entry(path: &str, hash: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|component| component == "..")
        && is_sha256(hash)
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
