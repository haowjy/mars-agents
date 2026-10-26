//! Host-independent package tree access.

use std::collections::{BTreeMap, BTreeSet};

/// Read access to a package tree through normalized package-relative paths.
///
/// Paths use `/` separators, are never absolute, and contain no empty, `.` or
/// `..` components. The empty path names the tree root for directory queries.
pub trait TreeSource {
    fn read(&self, path: &str) -> Option<&[u8]>;
    fn is_file(&self, path: &str) -> bool;
    fn is_dir(&self, path: &str) -> bool;

    /// Return the direct child paths of `dir` in stable lexical order.
    fn children(&self, dir: &str) -> Vec<String>;
}

/// An in-memory package tree for WASM callers and fixtures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTree {
    files: BTreeMap<String, Vec<u8>>,
}

impl FileTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, TreePathError> {
        let mut tree = Self::new();
        for (path, bytes) in files {
            tree.insert(path, bytes)?;
        }
        Ok(tree)
    }

    pub fn insert(
        &mut self,
        path: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), TreePathError> {
        let path = path.into();
        validate_file_path(&path)?;
        self.files.insert(path, bytes.into());
        Ok(())
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

impl TreeSource for FileTree {
    fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    fn is_file(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        if path.is_empty() {
            return true;
        }
        let prefix = format!("{path}/");
        self.files.keys().any(|file| file.starts_with(&prefix))
    }

    fn children(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        self.files
            .keys()
            .filter_map(|file| {
                let remainder = file.strip_prefix(&prefix)?;
                let child = remainder.split('/').next()?;
                Some(if dir.is_empty() {
                    child.to_owned()
                } else {
                    format!("{dir}/{child}")
                })
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid package tree path `{path}`: {reason}")]
pub struct TreePathError {
    pub path: String,
    pub reason: &'static str,
}

fn validate_file_path(path: &str) -> Result<(), TreePathError> {
    let invalid = |reason| TreePathError {
        path: path.to_owned(),
        reason,
    };
    if path.is_empty() {
        return Err(invalid("a file path cannot be empty"));
    }
    if path.starts_with('/') {
        return Err(invalid("paths must be package-relative"));
    }
    if path.contains('\\') {
        return Err(invalid("paths must use `/` separators"));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(invalid("paths must be normalized"));
    }
    Ok(())
}
