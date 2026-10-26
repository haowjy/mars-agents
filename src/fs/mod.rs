use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;

/// Top-level source entries excluded when installing flat skill repositories.
pub const FLAT_SKILL_EXCLUDED_TOP_LEVEL: &[&str] = &[
    ".git",
    ".mars",
    "mars.toml",
    "mars.lock",
    "mars.local.toml",
    "mars.curated.toml",
    "mars.curated.local.toml",
    ".gitignore",
];

/// Resolve symlinks in the existing prefix of a path while retaining its missing tail.
pub(crate) fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, MarsError> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }

    let mut existing = normalized.as_path();
    let mut missing = Vec::new();
    loop {
        match dunce::canonicalize(existing) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let component = existing.file_name().ok_or_else(|| MarsError::Io {
                    operation: "resolve path".to_string(),
                    path: normalized.clone(),
                    source,
                })?;
                missing.push(component.to_os_string());
                existing = existing.parent().ok_or_else(|| MarsError::InvalidRequest {
                    message: format!("cannot resolve path {}", normalized.display()),
                })?;
            }
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "resolve path".to_string(),
                    path: existing.to_path_buf(),
                    source,
                });
            }
        }
    }
}

/// Atomic file write: write to temp file in same directory, then rename.
///
/// The rename is atomic on POSIX. Temp files are in the same directory
/// as the destination to guarantee same-filesystem atomic rename.
pub fn atomic_write(dest: &Path, content: &[u8]) -> Result<(), MarsError> {
    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let parent = dest.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(content)?;
    tmp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    tmp.persist(dest).map_err(|e| e.error)?;
    Ok(())
}

/// Atomically replace a copied file's bytes without changing its permissions.
pub(crate) fn atomic_write_preserving_permissions(
    dest: &Path,
    content: &[u8],
) -> Result<(), MarsError> {
    let permissions = fs::metadata(dest)?.permissions();
    #[cfg(windows)]
    clear_readonly(dest)?;
    atomic_write(dest, content)?;
    fs::set_permissions(dest, permissions)?;
    Ok(())
}

/// Atomically write a regular file unless its bytes are already identical.
///
/// Returns `true` when a write occurred. Symlinks and non-regular destinations
/// are always replaced rather than treated as managed output.
pub fn atomic_write_if_changed(dest: &Path, content: &[u8]) -> Result<bool, MarsError> {
    let unchanged = dest
        .symlink_metadata()
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .and_then(|_| fs::read(dest).ok())
        .is_some_and(|existing| existing == content);
    if unchanged {
        return Ok(false);
    }
    atomic_write(dest, content)?;
    Ok(true)
}

/// Atomic directory install: copy source tree to a temp dir in the same
/// parent as `dest`, then rename into place.
///
/// Uses rename-old-then-rename-new to minimize the window where `dest`
/// doesn't exist. If `dest` already exists, it's renamed to `.{name}.old`
/// before the new content takes its place. Stale `.old` from prior crashes
/// is cleaned up automatically.
pub fn atomic_install_dir(src: &Path, dest: &Path) -> Result<(), MarsError> {
    atomic_install_dir_impl(src, dest, &[])
}

/// Atomic directory install with optional top-level source entry exclusions.
pub fn atomic_install_dir_filtered(
    src: &Path,
    dest: &Path,
    excluded_top_level: &[&str],
) -> Result<(), MarsError> {
    atomic_install_dir_impl(src, dest, excluded_top_level)
}

fn atomic_install_dir_impl(
    src: &Path,
    dest: &Path,
    excluded_top_level: &[&str],
) -> Result<(), MarsError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    let tmp_dir = tempfile::TempDir::new_in(parent)?;
    copy_dir_recursive(src, tmp_dir.path(), src, &|relative| {
        is_excluded_top_level(relative, excluded_top_level)
    })?;
    install_prepared_temp_dir(tmp_dir, dest)
}

fn install_prepared_temp_dir(tmp_dir: tempfile::TempDir, dest: &Path) -> Result<(), MarsError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let tmp_path = tmp_dir.keep();

    if dest.exists() {
        // Step 1: Rename old to .old (old content still accessible)
        let old_path = parent.join(format!(
            ".{}.old",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
        // Clean up stale .old from a prior crash
        if old_path.exists() {
            fs::remove_dir_all(&old_path)?;
        }
        // Atomic: old content moves to .old, dest slot is free
        fs::rename(dest, &old_path)?;
        // Atomic: new content takes dest slot
        if let Err(e) = fs::rename(&tmp_path, dest) {
            // Rollback: move old content back
            let _ = fs::rename(&old_path, dest);
            let _ = fs::remove_dir_all(&tmp_path);
            return Err(e.into());
        }
        // Cleanup: remove old content (non-critical)
        let _ = fs::remove_dir_all(&old_path);
    } else {
        fs::rename(&tmp_path, dest)?;
    }

    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(
    src: &Path,
    dest: &Path,
    root: &Path,
    excluded: &impl Fn(&Path) -> bool,
) -> Result<(), MarsError> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        let rel_path = src_path
            .strip_prefix(root)
            .expect("copy traversal path should be under root");
        if excluded(rel_path) {
            continue;
        }
        if file_type.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_dir_recursive(&src_path, &dest_path, root, excluded)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// Copy a derived regular tree while omitting selected relative subtrees.
pub(crate) fn copy_dir_filtered(
    src: &Path,
    dest: &Path,
    excluded_paths: &[PathBuf],
) -> Result<(), MarsError> {
    fs::create_dir_all(dest)?;
    copy_dir_recursive(src, dest, src, &|relative| {
        excluded_paths
            .iter()
            .any(|excluded| relative.starts_with(excluded))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthoredEntryKind {
    Directory,
    File,
}

#[derive(Debug)]
pub(crate) struct AuthoredTreeEntry {
    pub source: PathBuf,
    pub relative: PathBuf,
    pub kind: AuthoredEntryKind,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AuthoredSymlinkPolicy<'a> {
    /// Preserve sync's historical behavior: file links are followed when copied.
    Follow,
    /// Materialize links only when their targets remain inside this package root.
    ConfineTo(&'a Path),
}

/// Walk one authored source tree in deterministic parent-before-child order.
///
/// Generated plugin trees are source-boundary metadata, so this is deliberately
/// separate from general copy/hash primitives, which remain byte-faithful.
pub(crate) fn authored_tree_entries(
    source_root: &Path,
    excluded_paths: &[PathBuf],
    symlink_policy: AuthoredSymlinkPolicy<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<AuthoredTreeEntry>, MarsError> {
    let (walk_root, package_root, mut ancestors) = match symlink_policy {
        AuthoredSymlinkPolicy::Follow => (source_root.to_path_buf(), None, None),
        AuthoredSymlinkPolicy::ConfineTo(package_root) => {
            let package_root =
                dunce::canonicalize(package_root).map_err(|source| MarsError::Io {
                    operation: "resolve published package root".to_string(),
                    path: package_root.to_path_buf(),
                    source,
                })?;
            let resolved_source = resolve_published_path(&package_root, source_root)?;
            if !fs::metadata(&resolved_source)?.is_dir() {
                return Err(invalid_published_entry(
                    source_root,
                    "source tree is not a directory",
                ));
            }
            let ancestors = std::collections::HashSet::from([resolved_source.clone()]);
            (resolved_source, Some(package_root), Some(ancestors))
        }
    };
    if crate::plugin_metadata::has_valid_plugin_sentinel(&walk_root) {
        report_generated_tree(source_root, diag);
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    collect_authored_tree_entries(
        package_root.as_deref(),
        source_root,
        &walk_root,
        Path::new(""),
        excluded_paths,
        diag,
        ancestors.as_mut(),
        &mut entries,
    )?;
    Ok(entries)
}

/// Resolve one authored file for publication without following it outside the
/// package that owns it.
pub(crate) fn strict_authored_file(
    package_root: &Path,
    source_path: &Path,
) -> Result<PathBuf, MarsError> {
    let package_root = dunce::canonicalize(package_root).map_err(|source| MarsError::Io {
        operation: "resolve published package root".to_string(),
        path: package_root.to_path_buf(),
        source,
    })?;
    let resolved = resolve_published_path(&package_root, source_path)?;
    if !fs::metadata(&resolved)?.is_file() {
        return Err(invalid_published_entry(
            source_path,
            "entry is not a regular file",
        ));
    }
    Ok(resolved)
}

#[allow(clippy::too_many_arguments)]
fn collect_authored_tree_entries(
    package_root: Option<&Path>,
    logical_root: &Path,
    actual_dir: &Path,
    relative_dir: &Path,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
    mut ancestors: Option<&mut std::collections::HashSet<PathBuf>>,
    entries: &mut Vec<AuthoredTreeEntry>,
) -> Result<(), MarsError> {
    let mut children = fs::read_dir(actual_dir)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let relative = relative_dir.join(child.file_name());
        if excluded_paths
            .iter()
            .any(|excluded| relative.starts_with(excluded))
        {
            continue;
        }

        let logical_path = logical_root.join(&relative);
        let file_type = child.file_type()?;
        let confined_link = file_type.is_symlink().then_some(package_root).flatten();
        let actual_path = match confined_link {
            Some(package_root) => resolve_published_path(package_root, &logical_path)?,
            None => child.path(),
        };
        let metadata = match confined_link {
            Some(_) => fs::metadata(&actual_path)?,
            None => child.metadata()?,
        };

        let is_directory = if package_root.is_some() {
            metadata.is_dir()
        } else {
            file_type.is_dir()
        };
        if is_directory {
            if crate::plugin_metadata::has_valid_plugin_sentinel(&actual_path) {
                report_generated_tree(&logical_path, diag);
                continue;
            }
            let walked_dir = if package_root.is_some() {
                dunce::canonicalize(&actual_path).map_err(|source| MarsError::Io {
                    operation: "resolve published source directory".to_string(),
                    path: logical_path.clone(),
                    source,
                })?
            } else {
                actual_path
            };
            if let Some(ancestors) = ancestors.as_mut()
                && !ancestors.insert(walked_dir.clone())
            {
                return Err(invalid_published_entry(
                    &logical_path,
                    "directory symlink creates a cycle",
                ));
            }
            entries.push(AuthoredTreeEntry {
                source: walked_dir.clone(),
                relative: relative.clone(),
                kind: AuthoredEntryKind::Directory,
            });
            collect_authored_tree_entries(
                package_root,
                logical_root,
                &walked_dir,
                &relative,
                excluded_paths,
                diag,
                ancestors.as_deref_mut(),
                entries,
            )?;
            if let Some(ancestors) = ancestors.as_mut() {
                ancestors.remove(&walked_dir);
            }
        } else if package_root.is_none() || metadata.is_file() {
            entries.push(AuthoredTreeEntry {
                source: actual_path,
                relative,
                kind: AuthoredEntryKind::File,
            });
        } else {
            return Err(invalid_published_entry(
                &logical_path,
                "entry is not a regular file or directory",
            ));
        }
    }
    Ok(())
}

fn resolve_published_path(package_root: &Path, path: &Path) -> Result<PathBuf, MarsError> {
    let resolved = dunce::canonicalize(path).map_err(|source| {
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            invalid_published_entry(
                path,
                &format!("symlink target cannot be resolved: {source}"),
            )
        } else {
            MarsError::Io {
                operation: "resolve published source path".to_string(),
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    if !resolved.starts_with(package_root) {
        return Err(invalid_published_entry(
            path,
            &format!(
                "symlink target {} leaves package root {}",
                resolved.display(),
                package_root.display()
            ),
        ));
    }
    Ok(resolved)
}

fn invalid_published_entry(path: &Path, reason: &str) -> MarsError {
    MarsError::InvalidRequest {
        message: format!(
            "published source path {} is unsafe: {reason}",
            path.display()
        ),
    }
}

pub(crate) fn report_generated_tree(path: &Path, diag: &mut DiagnosticCollector) {
    let path = path.components().collect::<PathBuf>();
    diag.info_once(
        "generated-plugin-tree-skipped",
        format!(
            "skipping generated plugin tree at `{}` while reading authored package input",
            path.display()
        ),
    );
}

fn is_excluded_top_level(path: &Path, excluded_top_level: &[&str]) -> bool {
    let Some(first) = path.components().next().map(|c| c.as_os_str()) else {
        return false;
    };
    excluded_top_level.iter().any(|excluded| first == *excluded)
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
pub fn clear_readonly(path: &Path) -> std::io::Result<()> {
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut perms = metadata.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms)?;
        }
    }
    Ok(())
}

/// Advisory file lock (flock) for concurrent access.
///
/// Prevents concurrent `mars sync` from corrupting state.
/// The lock is held start-to-end — acquired before fetching and held through completion.
/// Dropping the `FileLock` closes the fd, which releases the advisory lock.
pub struct FileLock {
    _fd: fs::File,
}

impl FileLock {
    /// Acquire an advisory file lock, blocking until available.
    pub fn acquire(lock_path: &Path) -> Result<Self, MarsError> {
        let file = Self::open_lock_file(lock_path)?;
        platform::lock_exclusive(&file)?;
        Ok(FileLock { _fd: file })
    }

    /// Acquire the same lock without waiting.
    ///
    /// `Ok(None)` means another holder has it. Every other failure is an error:
    /// a caller that reads a broken lock file as contention cannot tell the two
    /// apart and silently skips its work forever.
    pub fn try_acquire(lock_path: &Path) -> Result<Option<Self>, MarsError> {
        let file = Self::open_lock_file(lock_path)?;
        match platform::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(FileLock { _fd: file })),
            Err(error) if platform::is_lock_held(&error) => Ok(None),
            Err(source) => Err(MarsError::Io {
                operation: "lock".to_string(),
                path: lock_path.to_path_buf(),
                source,
            }),
        }
    }

    /// Open (or create) the lock file, creating parent dirs if needed.
    fn open_lock_file(lock_path: &Path) -> Result<fs::File, MarsError> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        Ok(file)
    }
}

#[cfg(unix)]
mod platform {
    use std::fs;
    use std::os::unix::io::AsRawFd;

    pub fn lock_exclusive(file: &fs::File) -> std::io::Result<()> {
        // SAFETY: the file descriptor is valid while `file` is alive.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if ret != 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn try_lock_exclusive(file: &fs::File) -> std::io::Result<()> {
        // SAFETY: the file descriptor is valid while `file` is alive.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn is_lock_held(error: &std::io::Error) -> bool {
        error.raw_os_error() == Some(libc::EWOULDBLOCK)
    }
}

#[cfg(windows)]
mod platform {
    use std::fs;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };

    pub fn lock_exclusive(file: &fs::File) -> std::io::Result<()> {
        lock(file, LOCKFILE_EXCLUSIVE_LOCK)
    }

    pub fn try_lock_exclusive(file: &fs::File) -> std::io::Result<()> {
        lock(file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)
    }

    pub fn is_lock_held(error: &std::io::Error) -> bool {
        error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32)
    }

    /// Lock the whole file at offset 0, the range every Mars lock uses.
    fn lock(file: &fs::File, flags: u32) -> std::io::Result<()> {
        let handle = file.as_raw_handle() as HANDLE;
        // SAFETY: zero-initialized OVERLAPPED is accepted by LockFileEx for
        // whole-file locks at offset 0.
        let mut overlapped = unsafe { std::mem::zeroed() };
        // SAFETY: handle is valid while `file` is alive and `overlapped` outlives the call.
        let ret = unsafe { LockFileEx(handle, flags, 0, !0, !0, &mut overlapped) };
        if ret == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("nested").join("dir").join("file.txt");
        let content = b"nested content";

        atomic_write(&dest, content).unwrap();

        assert_eq!(fs::read(&dest).unwrap(), content);
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("output.txt");

        atomic_write(&dest, b"first").unwrap();
        atomic_write(&dest, b"second").unwrap();

        assert_eq!(fs::read(&dest).unwrap(), b"second");
    }

    #[test]
    fn atomic_install_dir_copies_tree() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src_dir");
        let dest = dir.path().join("dest_dir");

        // Create source tree
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "file a").unwrap();
        fs::write(src.join("sub").join("b.txt"), "file b").unwrap();

        atomic_install_dir(&src, &dest).unwrap();

        assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "file a");
        assert_eq!(
            fs::read_to_string(dest.join("sub").join("b.txt")).unwrap(),
            "file b"
        );
    }

    #[test]
    fn atomic_install_dir_replaces_existing() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src_dir");
        let dest = dir.path().join("dest_dir");

        // Create initial dest
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old.txt"), "old").unwrap();

        // Create source
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();

        atomic_install_dir(&src, &dest).unwrap();

        assert!(dest.join("new.txt").exists());
        assert!(!dest.join("old.txt").exists());
    }

    #[test]
    fn atomic_install_dir_cleans_stale_old() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src_dir");
        let dest = dir.path().join("dest_dir");

        // Create initial dest
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old.txt"), "old").unwrap();

        // Create stale .old from a prior crash
        let old_path = dir.path().join(".dest_dir.old");
        fs::create_dir_all(&old_path).unwrap();
        fs::write(old_path.join("stale.txt"), "stale").unwrap();

        // Create source
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("new.txt"), "new").unwrap();

        atomic_install_dir(&src, &dest).unwrap();

        assert!(dest.join("new.txt").exists());
        assert!(!dest.join("old.txt").exists());
        assert!(!old_path.exists(), "stale .old should be cleaned up");
    }

    #[test]
    fn atomic_install_dir_filtered_excludes_top_level_entries() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src_dir");
        let dest = dir.path().join("dest_dir");

        fs::create_dir_all(src.join(".git")).unwrap();
        fs::create_dir_all(src.join("resources")).unwrap();
        fs::write(src.join("SKILL.md"), "skill").unwrap();
        fs::write(src.join("mars.toml"), "ignored").unwrap();
        fs::write(src.join("mars.curated.toml"), "ignored").unwrap();
        fs::write(src.join("mars.curated.local.toml"), "ignored").unwrap();
        fs::write(src.join(".gitignore"), "ignored").unwrap();
        fs::write(src.join(".git").join("config"), "ignored").unwrap();
        fs::write(src.join("resources").join("guide.md"), "kept").unwrap();

        atomic_install_dir_filtered(&src, &dest, FLAT_SKILL_EXCLUDED_TOP_LEVEL).unwrap();

        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join("resources").join("guide.md").exists());
        assert!(!dest.join(".git").exists());
        assert!(!dest.join("mars.toml").exists());
        assert!(!dest.join("mars.curated.toml").exists());
        assert!(!dest.join("mars.curated.local.toml").exists());
        assert!(!dest.join(".gitignore").exists());
    }

    #[test]
    fn file_lock_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("nested").join("dir").join("test.lock");

        let lock = FileLock::acquire(&lock_path).unwrap();
        assert!(lock_path.exists());
        drop(lock);
    }
}
