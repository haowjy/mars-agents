//! Shared metadata for Mars-owned generated plugin output directories.

use std::path::Path;

use mars_schema::plugin_metadata::{PluginSentinel, SENTINEL_FILE};

use crate::error::MarsError;

/// The Mars that writes a sentinel's `mars_version`, and the one `mars check` compares it with.
pub(crate) const MARS_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn validate(sentinel: &PluginSentinel, path: &Path) -> Result<(), MarsError> {
    sentinel
        .validate()
        .map_err(|error| invalid(format!("{error}: {}", path.display())))
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
