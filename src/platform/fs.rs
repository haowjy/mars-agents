//! Atomic filesystem operations for durable writes and directory replacement.
//!
//! All durable Mars writes should go through this module.

use std::fs;
use std::path::Path;

use crate::error::MarsError;
use crate::fs::atomic_write;

#[cfg(windows)]
use crate::fs::clear_readonly;

/// Result of cache directory publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePublishResult {
    /// The directory was published (renamed from temp to destination).
    Published,
    /// The destination already existed; temp was removed.
    AlreadyPresent,
}

/// Replace a generated directory with rollback semantics.
pub fn replace_generated_dir(src: &Path, dest: &Path) -> Result<(), MarsError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io_context("create generated parent", parent, e))?;

    let old_path = parent.join(format!(
        ".{}.old",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));

    // Clean stale rollback content from prior crashes.
    if old_path.symlink_metadata().is_ok() {
        safe_remove(&old_path)?;
    }

    if dest.exists() {
        #[cfg(windows)]
        clear_readonly_recursive(dest)?;

        fs::rename(dest, &old_path)
            .map_err(|e| io_context("rename destination to backup", dest, e))?;

        if let Err(e) = fs::rename(src, dest) {
            let _ = fs::rename(&old_path, dest);
            let _ = safe_remove(src);
            return Err(io_context("rename source to destination", src, e));
        }

        let _ = safe_remove(&old_path);
    } else {
        fs::rename(src, dest).map_err(|e| io_context("rename source to destination", src, e))?;
    }

    Ok(())
}

/// Publish a cache directory iff destination is absent.
pub fn publish_cache_dir_if_absent(
    src: &Path,
    dest: &Path,
) -> Result<CachePublishResult, MarsError> {
    if dest.exists() {
        safe_remove(src)?;
        return Ok(CachePublishResult::AlreadyPresent);
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_context("create cache parent", parent, e))?;
    }

    match fs::rename(src, dest) {
        Ok(()) => Ok(CachePublishResult::Published),
        Err(_err) if dest.exists() => {
            let _ = safe_remove(src);
            Ok(CachePublishResult::AlreadyPresent)
        }
        Err(e) => Err(io_context("publish cache directory", src, e)),
    }
}

/// Remove a file or directory tree safely.
pub fn safe_remove(path: &Path) -> Result<(), MarsError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_context("read metadata for removal", path, e)),
    };

    #[cfg(windows)]
    if metadata.is_dir() {
        clear_readonly_recursive(path)?;
    } else {
        clear_readonly(path).map_err(|e| io_context("clear readonly bit", path, e))?;
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io_context("remove directory", path, e))?;
    } else {
        fs::remove_file(path).map_err(|e| io_context("remove file", path, e))?;
    }

    Ok(())
}

#[cfg(windows)]
fn clear_readonly_recursive(path: &Path) -> Result<(), MarsError> {
    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        clear_readonly(entry.path())
            .map_err(|e| io_context("clear readonly bit", entry.path(), e))?;
    }
    Ok(())
}

fn io_context(operation: &str, path: &Path, source: std::io::Error) -> MarsError {
    MarsError::Io {
        operation: operation.to_string(),
        path: path.to_path_buf(),
        source,
    }
}

/// Atomic file copy: read source (following symlinks), write to tmp, rename to dest.
pub fn atomic_copy_file(source: &Path, dest: &Path) -> Result<(), MarsError> {
    let content = fs::read(source)?;
    #[cfg(windows)]
    if dest.exists() {
        crate::fs::clear_readonly(dest)?;
    }
    atomic_write(dest, &content)
}

/// Atomic directory copy: deep copy source tree (following symlinks) to tmp, rename to dest.
pub fn atomic_copy_dir(source: &Path, dest: &Path) -> Result<(), MarsError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let tmp_dir = tempfile::TempDir::new_in(parent)?;
    copy_dir_following_symlinks(source, tmp_dir.path())?;
    let tmp_path = tmp_dir.keep();

    replace_generated_dir(&tmp_path, dest)
}

/// Whether two regular files have identical byte content.
///
/// Returns `false` (not an error) when either path is missing, not a regular file,
/// or a symlink — target sync installs copies, so symlink destinations must be rewritten.
pub fn file_content_equal(left: &Path, right: &Path) -> Result<bool, MarsError> {
    let left_meta = match fs::symlink_metadata(left) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    let right_meta = match fs::symlink_metadata(right) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if left_meta.file_type().is_symlink() || right_meta.file_type().is_symlink() {
        return Ok(false);
    }
    if !left_meta.is_file() || !right_meta.is_file() {
        return Ok(false);
    }
    Ok(fs::read(left)? == fs::read(right)?)
}

/// Whether two directory trees have identical structure (paths + entry kinds) and file bytes.
///
/// Returns `false` when either root is a symlink, any nested entry is a symlink or
/// non-regular type, directory structure differs (including empty directories), or
/// any regular file's bytes differ.
pub fn directory_trees_content_equal(left: &Path, right: &Path) -> Result<bool, MarsError> {
    let left_meta = match fs::symlink_metadata(left) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    let right_meta = match fs::symlink_metadata(right) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if left_meta.file_type().is_symlink() || right_meta.file_type().is_symlink() {
        return Ok(false);
    }
    if !left_meta.is_dir() || !right_meta.is_dir() {
        return Ok(false);
    }

    let left_entries = match collect_relative_tree_entries(left, left)? {
        TreeCollectOutcome::Reject => return Ok(false),
        TreeCollectOutcome::Entries(entries) => entries,
    };
    let right_entries = match collect_relative_tree_entries(right, right)? {
        TreeCollectOutcome::Reject => return Ok(false),
        TreeCollectOutcome::Entries(entries) => entries,
    };
    if left_entries != right_entries {
        return Ok(false);
    }
    for entry in &left_entries {
        if entry.kind == TreeEntryKind::File
            && fs::read(left.join(&entry.rel_path))? != fs::read(right.join(&entry.rel_path))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TreeEntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TreeEntry {
    rel_path: String,
    kind: TreeEntryKind,
}

/// Outcome of walking a directory tree for equality comparison.
enum TreeCollectOutcome {
    /// Symlink or other non-regular entry — trees must be considered unequal.
    Reject,
    Entries(Vec<TreeEntry>),
}

fn collect_relative_tree_entries(
    root: &Path,
    current: &Path,
) -> Result<TreeCollectOutcome, MarsError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path.strip_prefix(root).expect("path is always under root");
        let rel_path: String = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        if file_type.is_symlink() {
            return Ok(TreeCollectOutcome::Reject);
        }
        if file_type.is_dir() {
            entries.push(TreeEntry {
                rel_path: rel_path.clone(),
                kind: TreeEntryKind::Directory,
            });
            match collect_relative_tree_entries(root, &path)? {
                TreeCollectOutcome::Reject => return Ok(TreeCollectOutcome::Reject),
                TreeCollectOutcome::Entries(mut nested) => entries.append(&mut nested),
            }
        } else if file_type.is_file() {
            entries.push(TreeEntry {
                rel_path,
                kind: TreeEntryKind::File,
            });
        } else {
            return Ok(TreeCollectOutcome::Reject);
        }
    }
    entries.sort();
    Ok(TreeCollectOutcome::Entries(entries))
}

/// Recursively copy a directory, following symlinks on the source side.
///
/// Uses `fs::metadata` (not `symlink_metadata`) to follow symlinks.
/// Files are copied with plain `fs::read`+`fs::write` because the destination
/// is inside a temp dir — the atomicity guarantee comes from the final rename
/// of the enclosing temp dir, not from per-file atomics.
fn copy_dir_following_symlinks(source: &Path, dest: &Path) -> Result<(), MarsError> {
    fs::create_dir_all(dest)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        // Follow symlinks — fs::metadata resolves through symlinks
        let metadata = match fs::metadata(&source_path) {
            Ok(m) => m,
            Err(e) => {
                // If it's a broken symlink, give a descriptive error
                if entry.file_type()?.is_symlink() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("broken symlink in source tree: {}", source_path.display()),
                    )
                    .into());
                }
                return Err(e.into());
            }
        };

        if metadata.is_dir() {
            copy_dir_following_symlinks(&source_path, &dest_path)?;
        } else if metadata.is_file() {
            let content = fs::read(&source_path)?;
            fs::write(&dest_path, &content)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest_path, fs::Permissions::from_mode(0o644))?;
            }
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported filesystem entry: {}", source_path.display()),
            )
            .into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn replace_generated_dir_basic() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");

        fs::create_dir(&src).unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();

        replace_generated_dir(&src, &dest).unwrap();

        assert!(!src.exists());
        assert!(dest.join("file.txt").exists());
    }

    #[test]
    fn replace_generated_dir_replaces_existing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");

        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("old.txt"), "old").unwrap();

        fs::create_dir(&src).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();

        replace_generated_dir(&src, &dest).unwrap();

        assert!(!dest.join("old.txt").exists());
        assert!(dest.join("new.txt").exists());
    }

    #[test]
    fn publish_cache_dir_if_absent_publishes() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");

        fs::create_dir(&src).unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();

        let result = publish_cache_dir_if_absent(&src, &dest).unwrap();

        assert_eq!(result, CachePublishResult::Published);
        assert!(!src.exists());
        assert!(dest.join("file.txt").exists());
    }

    #[test]
    fn publish_cache_dir_if_absent_accepts_existing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");

        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("existing.txt"), "existing").unwrap();

        fs::create_dir(&src).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();

        let result = publish_cache_dir_if_absent(&src, &dest).unwrap();

        assert_eq!(result, CachePublishResult::AlreadyPresent);
        assert!(!src.exists());
        assert!(dest.join("existing.txt").exists());
        assert!(!dest.join("new.txt").exists());
    }

    #[test]
    fn safe_remove_handles_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent");

        safe_remove(&path).unwrap();
    }

    #[test]
    fn safe_remove_removes_file_and_directory_tree() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("file.txt");
        fs::write(&file, "content").unwrap();

        safe_remove(&file).unwrap();
        assert!(!file.exists());

        let dir = tmp.path().join("dir");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("nested").join("file.txt"), "content").unwrap();

        safe_remove(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn replace_generated_dir_cleans_stale_backup_before_replace() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        let old = tmp.path().join(".dest.old");

        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("old.txt"), "old").unwrap();
        fs::create_dir(&old).unwrap();
        fs::write(old.join("stale.txt"), "stale").unwrap();
        fs::create_dir(&src).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();

        replace_generated_dir(&src, &dest).unwrap();

        assert!(!old.exists());
        assert!(!dest.join("old.txt").exists());
        assert_eq!(fs::read_to_string(dest.join("new.txt")).unwrap(), "new");
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn directory_trees_content_equal_detects_identical_and_different_trees() {
        let dir = TempDir::new().expect("temp dir");
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        let other = dir.path().join("other");
        fs::create_dir_all(left.join("nested")).expect("create left");
        fs::create_dir_all(right.join("nested")).expect("create right");
        fs::create_dir_all(other.join("nested")).expect("create other");
        fs::write(left.join("root.txt"), "root").expect("write left root");
        fs::write(left.join("nested/child.txt"), "child").expect("write left child");
        fs::write(right.join("root.txt"), "root").expect("write right root");
        fs::write(right.join("nested/child.txt"), "child").expect("write right child");
        fs::write(other.join("root.txt"), "different").expect("write other root");
        fs::write(other.join("nested/child.txt"), "child").expect("write other child");

        assert!(directory_trees_content_equal(&left, &right).expect("compare equal"));
        assert!(!directory_trees_content_equal(&left, &other).expect("compare different"));
    }

    #[test]
    fn file_content_equal_compares_regular_files() {
        let dir = TempDir::new().expect("temp dir");
        let left = dir.path().join("left.txt");
        let right = dir.path().join("right.txt");
        let other = dir.path().join("other.txt");
        fs::write(&left, "same").expect("write left");
        fs::write(&right, "same").expect("write right");
        fs::write(&other, "different").expect("write other");

        assert!(file_content_equal(&left, &right).expect("compare equal"));
        assert!(!file_content_equal(&left, &other).expect("compare different"));
    }

    #[cfg(unix)]
    #[test]
    fn file_content_equal_rejects_symlink_dest_even_when_bytes_match() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join("target.txt");
        fs::write(&target, "same bytes").expect("write target");

        let regular = dir.path().join("regular.txt");
        fs::write(&regular, "same bytes").expect("write regular");

        let symlink = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &symlink).expect("create symlink");

        assert!(
            !file_content_equal(&regular, &symlink).expect("compare symlink dest"),
            "symlink dest must force rewrite even when target bytes match"
        );
    }

    #[test]
    fn directory_trees_content_equal_detects_empty_directory_delta() {
        let dir = TempDir::new().expect("temp dir");
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        fs::create_dir_all(left.join("empty-only")).expect("create left empty dir");
        fs::create_dir_all(&right).expect("create right root");
        fs::write(left.join("root.txt"), "root").expect("write left root");
        fs::write(right.join("root.txt"), "root").expect("write right root");

        assert!(
            !directory_trees_content_equal(&left, &right).expect("compare empty-dir delta"),
            "empty-directory-only structural delta must not compare equal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_trees_content_equal_rejects_symlink_entry() {
        let dir = TempDir::new().expect("temp dir");
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        let shared = dir.path().join("shared.txt");
        fs::write(&shared, "shared").expect("write shared");

        fs::create_dir_all(&left).expect("create left");
        fs::create_dir_all(&right).expect("create right");
        fs::write(left.join("root.txt"), "root").expect("write left root");
        fs::write(right.join("root.txt"), "root").expect("write right root");
        std::os::unix::fs::symlink(&shared, right.join("link.txt")).expect("create symlink");

        assert!(
            !directory_trees_content_equal(&left, &right).expect("compare symlink entry"),
            "symlink entry in tree must force rewrite"
        );
    }

    #[test]
    fn directory_trees_content_equal_identical_regular_file_trees_still_equal() {
        let dir = TempDir::new().expect("temp dir");
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        fs::create_dir_all(left.join("nested")).expect("create left nested");
        fs::create_dir_all(right.join("nested")).expect("create right nested");
        fs::write(left.join("root.txt"), "root").expect("write left root");
        fs::write(left.join("nested/child.txt"), "child").expect("write left child");
        fs::write(right.join("root.txt"), "root").expect("write right root");
        fs::write(right.join("nested/child.txt"), "child").expect("write right child");

        assert!(
            directory_trees_content_equal(&left, &right).expect("compare identical trees"),
            "identical all-regular-file trees must still compare equal"
        );
    }

    #[test]
    fn atomic_copy_file_copies_regular_file() {
        let dir = TempDir::new().expect("temp dir");
        let source = dir.path().join("source.txt");
        let dest = dir.path().join("dest").join("copied.txt");
        fs::write(&source, "hello").expect("write source");

        atomic_copy_file(&source, &dest).expect("copy file");

        assert_eq!(fs::read_to_string(dest).expect("read dest"), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_copy_file_follows_source_symlink() {
        let dir = TempDir::new().expect("temp dir");
        let real = dir.path().join("real.txt");
        fs::write(&real, "from-real").expect("write real");

        let source_link = dir.path().join("source-link.txt");
        std::os::unix::fs::symlink(&real, &source_link).expect("create symlink");

        let dest = dir.path().join("dest").join("copied.txt");
        atomic_copy_file(&source_link, &dest).expect("copy through symlink");

        let dest_meta = fs::symlink_metadata(&dest).expect("dest metadata");
        assert!(
            !dest_meta.file_type().is_symlink(),
            "dest should be a regular file"
        );
        assert_eq!(fs::read_to_string(dest).expect("read dest"), "from-real");
    }

    #[test]
    fn atomic_copy_dir_copies_tree() {
        let dir = TempDir::new().expect("temp dir");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::write(source.join("root.txt"), "root").expect("write root");
        fs::write(source.join("nested").join("child.txt"), "child").expect("write child");

        let dest = dir.path().join("dest");
        atomic_copy_dir(&source, &dest).expect("copy dir");

        assert_eq!(
            fs::read_to_string(dest.join("root.txt")).expect("read root"),
            "root"
        );
        assert_eq!(
            fs::read_to_string(dest.join("nested").join("child.txt")).expect("read child"),
            "child"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_copy_dir_follows_symlinks() {
        let dir = TempDir::new().expect("temp dir");
        let shared = dir.path().join("shared");
        fs::create_dir_all(shared.join("docs")).expect("create shared tree");
        fs::write(shared.join("docs").join("guide.md"), "guide").expect("write guide");
        fs::write(shared.join("main.txt"), "main").expect("write main");

        let source = dir.path().join("source");
        fs::create_dir_all(&source).expect("create source");
        std::os::unix::fs::symlink(shared.join("main.txt"), source.join("main-link.txt"))
            .expect("file symlink");
        std::os::unix::fs::symlink(shared.join("docs"), source.join("docs-link"))
            .expect("dir symlink");

        let dest = dir.path().join("dest");
        atomic_copy_dir(&source, &dest).expect("copy dir through symlinks");

        let main_meta = fs::symlink_metadata(dest.join("main-link.txt")).expect("main metadata");
        assert!(
            !main_meta.file_type().is_symlink(),
            "copied file entry should be regular"
        );
        assert_eq!(
            fs::read_to_string(dest.join("main-link.txt")).expect("read copied main"),
            "main"
        );

        let docs_meta = fs::symlink_metadata(dest.join("docs-link")).expect("docs metadata");
        assert!(
            !docs_meta.file_type().is_symlink(),
            "copied dir entry should be regular directory"
        );
        assert_eq!(
            fs::read_to_string(dest.join("docs-link").join("guide.md")).expect("read guide"),
            "guide"
        );
    }
}
