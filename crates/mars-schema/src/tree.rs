//! Host-independent package tree access.

use std::collections::{BTreeMap, BTreeSet};

/// Read access to a package tree through normalized package-relative paths.
///
/// Paths use `/` separators, are never absolute, and contain no empty, `.` or
/// `..` components. The empty path names the tree root for directory queries.
pub trait TreeSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    fn is_file(&self, path: &str) -> bool;
    fn is_dir(&self, path: &str) -> bool;

    /// Return the direct child paths of `dir` in stable lexical order.
    fn children(&self, dir: &str) -> Vec<String>;
}

/// An in-memory package tree for WASM callers and fixtures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTree {
    files: BTreeMap<String, Vec<u8>>,
    children: BTreeMap<String, BTreeSet<String>>,
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
        let components = path.split('/').collect::<Vec<_>>();
        let has_file_ancestor =
            (1..components.len()).any(|end| self.files.contains_key(&components[..end].join("/")));
        let descendant_prefix = format!("{path}/");
        let has_file_descendant = self
            .files
            .range(path.clone()..)
            .next()
            .is_some_and(|(file, _)| file.starts_with(&descendant_prefix));
        if has_file_ancestor || has_file_descendant {
            return Err(TreePathError {
                path,
                reason: "a path cannot be both a file and a directory",
            });
        }
        index_children(&mut self.children, &path);
        self.files.insert(path, bytes.into());
        Ok(())
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

impl TreeSource for FileTree {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }

    fn is_file(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.children.contains_key(path)
    }

    fn children(&self, dir: &str) -> Vec<String> {
        self.children
            .get(dir)
            .map(|children| children.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn index_children(index: &mut BTreeMap<String, BTreeSet<String>>, path: &str) {
    let components = path.split('/').collect::<Vec<_>>();
    for child_index in 0..components.len() {
        let parent = components[..child_index].join("/");
        let child = components[..=child_index].join("/");
        index.entry(parent).or_default().insert(child);
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
