//! Path source adapter — resolves local filesystem paths.
//!
//! Path sources are always "live" — no caching, no copying.
//! Returns the resolved path directly.

use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::source::ResolvedRef;
use crate::types::SourceName;

/// Fetch a path source: resolve relative paths against project root, verify exists.
///
/// - Relative paths are resolved against `project_root`.
/// - Absolute paths are used as-is.
/// - The path must exist and be a directory.
/// - Returns `ResolvedRef` with no version/commit (path sources are unversioned).
pub fn fetch_path(
    path: &Path,
    project_root: &Path,
    source_name: &str,
) -> Result<ResolvedRef, MarsError> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };

    // Canonicalize to resolve symlinks and `..` components
    let resolved = canonicalize_path(&resolved, source_name)?;

    // Verify the path exists and is a directory
    if !resolved.exists() {
        return Err(MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("path does not exist: {}", resolved.display()),
        });
    }

    if !resolved.is_dir() {
        return Err(MarsError::Source {
            source_name: source_name.to_string(),
            message: format!("path is not a directory: {}", resolved.display()),
        });
    }

    Ok(ResolvedRef {
        source_name: SourceName::from(source_name),
        version: None,
        version_tag: None,
        commit: None,
        tree_path: resolved,
    })
}

/// Canonicalize a path, providing a helpful error on failure.
fn canonicalize_path(path: &Path, source_name: &str) -> Result<PathBuf, MarsError> {
    dunce::canonicalize(path).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("failed to resolve path `{}`: {e}", path.display()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fetch_invalid_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let err = fetch_path(&dir.path().join("nonexistent"), dir.path(), "bad-source")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("bad-source"),
            "error should mention source name: {err}"
        );
        assert!(
            err.contains("nonexistent"),
            "error should mention the path: {err}"
        );

        let file_path = dir.path().join("not-a-dir.txt");
        std::fs::write(&file_path, "content").unwrap();
        let err = fetch_path(&file_path, dir.path(), "file-source")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not a directory"),
            "error should mention 'not a directory': {err}"
        );
    }
}
