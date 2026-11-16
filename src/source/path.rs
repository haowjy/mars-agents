//! Path source adapter — resolves local filesystem paths.
//!
//! Path sources are always "live" — no caching, no copying.

use std::path::{Path, PathBuf};

use crate::error::MarsError;

/// The canonical directory `path` names (a relative one is relative to `project_root`).
pub fn fetch_path(
    path: &Path,
    project_root: &Path,
    source_name: &str,
) -> Result<PathBuf, MarsError> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };

    let resolved = canonicalize_path(&resolved, source_name)?;

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

    Ok(resolved)
}

fn canonicalize_path(path: &Path, source_name: &str) -> Result<PathBuf, MarsError> {
    dunce::canonicalize(path).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!("failed to resolve path `{}`: {e}", path.display()),
    })
}
