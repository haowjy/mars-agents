//! Read a content directory into a `FileTree`.

use std::path::Path;

use mars_schema::tree::{FileTree, TreeSource};

use crate::error::MarsError;
use crate::package_prepare::DiskTreeSource;

/// Every file below `root`, read through `DiskTreeSource`: an entry that escapes `root` or a name
/// that is not UTF-8 fails the read, as it fails a package prepared from the same directory.
///
/// A symlinked directory is followed, as it is when a package is prepared, unless it leads back
/// to a directory the walk is inside. Empty directories are not kept: `FileTree` holds files.
pub(crate) fn read_tree(root: &Path) -> Result<FileTree, MarsError> {
    let source = DiskTreeSource::new(root)?;
    let mut tree = FileTree::new();
    let mut inside = vec![canonical(root)?];
    walk(&source, root, "", &mut inside, &mut tree)?;
    match source.take_error() {
        Some(error) => Err(error),
        None => Ok(tree),
    }
}

fn walk(
    source: &DiskTreeSource,
    root: &Path,
    dir: &str,
    inside: &mut Vec<std::path::PathBuf>,
    tree: &mut FileTree,
) -> Result<(), MarsError> {
    for child in source.children(dir) {
        if source.is_dir(&child) {
            let real = canonical(&root.join(&child))?;
            if inside.contains(&real) {
                continue;
            }
            inside.push(real);
            walk(source, root, &child, inside, tree)?;
            inside.pop();
        } else if let Some(bytes) = source.read(&child) {
            tree.insert(child.clone(), bytes)
                .map_err(|error| MarsError::InvalidRequest {
                    message: error.to_string(),
                })?;
        }
    }
    Ok(())
}

fn canonical(path: &Path) -> Result<std::path::PathBuf, MarsError> {
    dunce::canonicalize(path).map_err(|source| MarsError::Io {
        operation: "resolve package directory".to_string(),
        path: path.to_path_buf(),
        source,
    })
}
