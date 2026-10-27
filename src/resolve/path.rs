use std::path::Path;

use crate::error::MarsError;
use crate::types::SourceName;
use mars_schema::names::SourceSubpath;

use super::types::RootedSourceRef;

pub(crate) fn apply_subpath(
    source_name: &SourceName,
    checkout_root: &Path,
    subpath: Option<&SourceSubpath>,
) -> Result<RootedSourceRef, MarsError> {
    let package_root = match subpath {
        Some(subpath) => {
            subpath
                .join_under(checkout_root)
                .map_err(|_| MarsError::SubpathTraversal {
                    source_name: source_name.to_string(),
                    subpath: subpath.to_string(),
                    checkout_root: checkout_root.to_path_buf(),
                })?
        }
        None => checkout_root.to_path_buf(),
    };

    if !package_root.exists() {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathMissing {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root does not exist under checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    if !package_root.is_dir() {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathNotDirectory {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root is not a directory under checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    let canonical_checkout = dunce::canonicalize(checkout_root).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!(
            "failed to canonicalize checkout root `{}`: {e}",
            checkout_root.display()
        ),
    })?;
    let canonical_package = dunce::canonicalize(&package_root).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!(
            "failed to canonicalize package root `{}`: {e}",
            package_root.display()
        ),
    })?;

    if !canonical_package.starts_with(&canonical_checkout) {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathTraversal {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root escapes checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    Ok(RootedSourceRef {
        checkout_root: checkout_root.to_path_buf(),
        authored_root: package_root.clone(),
        package_root,
        plugin: None,
    })
}
