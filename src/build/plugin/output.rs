use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use crate::plugin_metadata::{PluginSentinel, SENTINEL_FILE};

/// Stage a Mars-owned plugin output directory without changing the live output.
///
/// `write` receives an empty sibling staging directory. The returned value owns
/// the output lock and removes the staging directory unless it is published.
pub(super) fn stage<T>(
    out_dir: &Path,
    write: impl FnOnce(&Path) -> Result<(PluginSentinel, T), MarsError>,
) -> Result<(StagedPlugin, T), MarsError> {
    let paths = OutputPaths::new(out_dir)?;
    let lock = crate::fs::FileLock::acquire(&paths.lock)?;

    validate_destination(&paths.out)?;
    fs::create_dir_all(&paths.parent)
        .map_err(|source| io_error("create plugin output parent", &paths.parent, source))?;
    recover_interrupted_swap(&paths)?;
    let output = classify_tree(&paths.out)?;
    ensure_replaceable(&paths.out, "plugin output", &output)?;

    // Recovery removes this path. Removing it again here would hide a race or
    // an invariant violation inside this process.
    fs::create_dir(&paths.new)
        .map_err(|source| io_error("create plugin staging directory", &paths.new, source))?;

    let staged = (|| {
        let (mut stored, value) = write(&paths.new)?;
        validate_staging_tree(&paths.new)?;

        let files = compute_file_hashes(&paths.new)?;
        stored.files = files;
        crate::plugin_metadata::validate(&stored, &paths.new.join(SENTINEL_FILE))?;
        write_sentinel(&paths.new, &stored)?;
        Ok(value)
    })();

    let value = match staged {
        Ok(value) => value,
        Err(build_error) => {
            return match crate::platform::fs::safe_remove(&paths.new) {
                Ok(()) => Err(build_error),
                Err(cleanup_error) => Err(MarsError::Internal(format!(
                    "plugin tree build failed: {build_error}; staging cleanup also failed: {cleanup_error}"
                ))),
            };
        }
    };

    Ok((StagedPlugin { paths, _lock: lock }, value))
}

/// Recover a plugin output after an interrupted stage or publish.
pub fn recover(out_dir: &Path) -> Result<(), MarsError> {
    let paths = OutputPaths::new(out_dir)?;
    let _lock = crate::fs::FileLock::acquire(&paths.lock)?;

    validate_destination(&paths.out)?;
    fs::create_dir_all(&paths.parent)
        .map_err(|source| io_error("create plugin output parent", &paths.parent, source))?;
    recover_interrupted_swap(&paths)
}

/// A complete plugin output waiting to replace the live directory.
pub(super) struct StagedPlugin {
    paths: OutputPaths,
    _lock: crate::fs::FileLock,
}

impl StagedPlugin {
    pub(super) fn publish(self) -> Result<(), MarsError> {
        match publish_staged_tree_inner(&self.paths) {
            Ok(()) => Ok(()),
            Err(error) => Err(clean_up_staging_after_error(&self.paths, error)),
        }
    }
}

impl Drop for StagedPlugin {
    fn drop(&mut self) {
        // Drop cannot report cleanup failures. A later stage runs recovery under
        // the same lock and retries this removal before it reads live output.
        let _ = crate::platform::fs::safe_remove(&self.paths.new);
    }
}

/// Recompute every regular output file hash except the sentinel.
pub(super) fn compute_file_hashes(
    out_dir: &Path,
) -> Result<std::collections::BTreeMap<String, String>, MarsError> {
    crate::hash::strict_file_hashes(out_dir, &[SENTINEL_FILE])
}

struct OutputPaths {
    out: PathBuf,
    parent: PathBuf,
    new: PathBuf,
    old: PathBuf,
    lock: PathBuf,
}

impl OutputPaths {
    fn new(out_dir: &Path) -> Result<Self, MarsError> {
        if out_dir.as_os_str().is_empty() {
            return Err(invalid("plugin output path cannot be empty"));
        }
        if matches!(
            out_dir.components().next_back(),
            Some(std::path::Component::ParentDir)
        ) {
            return Err(invalid(format!(
                "plugin output cannot end in `..`; choose a dedicated output directory instead of {}",
                out_dir.display()
            )));
        }

        let out = std::path::absolute(out_dir)
            .map_err(|source| io_error("resolve plugin output path", out_dir, source))?;
        let file_name = out
            .file_name()
            .ok_or_else(|| {
                invalid(format!(
                    "plugin output cannot be a filesystem root: {}",
                    out.display()
                ))
            })?
            .to_os_string();
        if file_name.to_str().is_none() {
            return Err(invalid(format!(
                "plugin output path must be valid Unicode: {}",
                out.display()
            )));
        }
        let parent = out
            .parent()
            .ok_or_else(|| {
                invalid(format!(
                    "plugin output has no parent directory: {}",
                    out.display()
                ))
            })?
            .to_path_buf();

        let identity = canonicalize_existing_prefix(&out)?;
        let identity_text = identity.to_str().ok_or_else(|| {
            invalid(format!(
                "plugin output path must be valid Unicode: {}",
                out.display()
            ))
        })?;
        #[cfg(windows)]
        let identity_text = identity_text.to_lowercase();
        #[cfg(not(windows))]
        let identity_text = identity_text.to_string();

        let lock_name = format!(
            "{}.lock",
            crate::platform::cache::safe_component_with_hash(&identity_text)
        );
        let cache_root = crate::platform::cache::global_cache_root()?;
        let cache_root = std::path::absolute(&cache_root)
            .map_err(|source| io_error("resolve Mars cache path", &cache_root, source))?;
        let lock = cache_root.join("plugin-output-locks").join(lock_name);
        let lock_identity = canonicalize_existing_prefix(&lock)?;
        if lock_identity.starts_with(&identity) {
            return Err(invalid(format!(
                "plugin output {} cannot contain its own Mars cache lock",
                out.display()
            )));
        }

        Ok(Self {
            new: parent.join(sibling_name(&file_name, ".mars-new")),
            old: parent.join(sibling_name(&file_name, ".mars-old")),
            out,
            parent,
            lock,
        })
    }
}

fn sibling_name(file_name: &std::ffi::OsStr, suffix: &str) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(suffix);
    name
}

fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, MarsError> {
    let mut existing = path;
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
                let component = existing
                    .file_name()
                    .ok_or_else(|| io_error("resolve plugin output path", existing, source))?;
                missing.push(component.to_os_string());
                existing = existing
                    .parent()
                    .ok_or_else(|| invalid(format!("cannot resolve path {}", path.display())))?;
            }
            Err(source) => {
                return Err(io_error("resolve plugin output path", existing, source));
            }
        }
    }
}

fn validate_destination(out_dir: &Path) -> Result<(), MarsError> {
    let identity = canonicalize_existing_prefix(out_dir)?;
    let cwd = std::env::current_dir()
        .and_then(dunce::canonicalize)
        .map_err(|source| io_error("resolve current directory", Path::new("."), source))?;
    if cwd.starts_with(&identity) {
        return Err(invalid(format!(
            "refusing to replace plugin output {} because it is the current directory or one of its ancestors",
            out_dir.display()
        )));
    }

    let metadata = match fs::symlink_metadata(out_dir) {
        Ok(metadata) => Some(metadata),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(io_error("inspect plugin output", out_dir, source));
        }
    };
    if let Some(metadata) = metadata {
        if metadata.file_type().is_symlink() {
            return Err(invalid(format!(
                "plugin output cannot be a symlink: {}",
                out_dir.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(invalid(format!(
                "plugin output must be a directory: {}",
                out_dir.display()
            )));
        }
        if path_exists(&out_dir.join("mars.toml"))? {
            return Err(invalid(format!(
                "refusing to replace package root {} as plugin output",
                out_dir.display()
            )));
        }
    }

    Ok(())
}

fn recover_interrupted_swap(paths: &OutputPaths) -> Result<(), MarsError> {
    remove_recovery_staging(&paths.new)?;

    let old = classify_tree(&paths.old)?;
    if matches!(old, TreeState::Missing) {
        return Ok(());
    }
    let output = classify_tree(&paths.out)?;

    match (old, output) {
        (TreeState::Empty | TreeState::Owned, TreeState::Owned) => {
            crate::platform::fs::safe_remove(&paths.old)
        }
        (TreeState::Empty | TreeState::Owned, TreeState::Missing) => {
            fs::rename(&paths.old, &paths.out)
                .map_err(|source| io_error("restore interrupted plugin output", &paths.old, source))
        }
        (old, output) => Err(invalid(format!(
            "refusing ambiguous plugin recovery at {}: rollback is {}, output is {}; both paths were preserved",
            paths.out.display(),
            old.description(),
            output.description()
        ))),
    }
}

fn remove_recovery_staging(path: &Path) -> Result<(), MarsError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("inspect plugin staging path", path, source)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(format!(
            "refusing plugin recovery because reserved staging path {} is {}; the path was preserved",
            path.display(),
            if metadata.file_type().is_symlink() {
                "a symlink"
            } else {
                "not a directory"
            }
        )));
    }
    crate::platform::fs::safe_remove(path)
}

#[derive(Debug)]
enum TreeState {
    Missing,
    Empty,
    Owned,
    Unowned(String),
}

impl TreeState {
    fn description(&self) -> &str {
        match self {
            Self::Missing => "missing",
            Self::Empty => "empty",
            Self::Owned => "Mars-owned",
            Self::Unowned(reason) => reason,
        }
    }
}

fn classify_tree(path: &Path) -> Result<TreeState, MarsError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TreeState::Missing);
        }
        Err(source) => return Err(io_error("inspect plugin directory", path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Ok(TreeState::Unowned("a symlink".to_string()));
    }
    if !metadata.is_dir() {
        return Ok(TreeState::Unowned("not a directory".to_string()));
    }

    let mut entries =
        fs::read_dir(path).map_err(|source| io_error("read plugin directory", path, source))?;
    match entries.next() {
        None => return Ok(TreeState::Empty),
        Some(Ok(_)) => {}
        Some(Err(source)) => return Err(io_error("read plugin directory", path, source)),
    }

    Ok(match crate::plugin_metadata::read(path) {
        Ok(_) => TreeState::Owned,
        Err(error) => TreeState::Unowned(error.to_string()),
    })
}

fn ensure_replaceable(path: &Path, description: &str, state: &TreeState) -> Result<(), MarsError> {
    match state {
        TreeState::Missing | TreeState::Empty | TreeState::Owned => Ok(()),
        TreeState::Unowned(reason) => Err(invalid(format!(
            "refusing to replace non-empty {description} {} without a valid Mars sentinel: {reason}",
            path.display()
        ))),
    }
}

fn validate_staging_tree(staging: &Path) -> Result<(), MarsError> {
    let metadata = fs::symlink_metadata(staging)
        .map_err(|source| io_error("inspect plugin staging directory", staging, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(format!(
            "plugin writer replaced its staging directory: {}",
            staging.display()
        )));
    }
    match staging.join(SENTINEL_FILE).symlink_metadata() {
        Ok(_) => Err(invalid(format!(
            "plugin writer must not create reserved file {SENTINEL_FILE}"
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(
            "inspect plugin sentinel path",
            &staging.join(SENTINEL_FILE),
            source,
        )),
    }
}

fn publish_staged_tree_inner(paths: &OutputPaths) -> Result<(), MarsError> {
    let rollback = classify_tree(&paths.old)?;
    if !matches!(rollback, TreeState::Missing) {
        return Err(invalid(format!(
            "refusing to publish plugin output {} because rollback path {} is {}",
            paths.out.display(),
            paths.old.display(),
            rollback.description()
        )));
    }
    let output = classify_tree(&paths.out)?;
    ensure_replaceable(&paths.out, "plugin output", &output)?;

    match output {
        TreeState::Empty | TreeState::Owned => {
            fs::rename(&paths.out, &paths.old).map_err(|source| {
                io_error("move current plugin output aside", &paths.out, source)
            })?;

            if let Err(source) = fs::rename(&paths.new, &paths.out) {
                let restore = fs::rename(&paths.old, &paths.out);
                if let Err(restore_error) = restore {
                    return Err(MarsError::Internal(format!(
                        "failed to publish plugin tree at {}: {source}; restoring the previous tree also failed: {restore_error}",
                        paths.out.display()
                    )));
                }
                return Err(io_error("publish plugin output", &paths.new, source));
            }

            crate::platform::fs::safe_remove(&paths.old)?;
        }
        TreeState::Missing => {
            fs::rename(&paths.new, &paths.out)
                .map_err(|source| io_error("publish plugin output", &paths.new, source))?;
        }
        TreeState::Unowned(_) => unreachable!("ensure_replaceable rejected unowned output"),
    }
    Ok(())
}

fn clean_up_staging_after_error(paths: &OutputPaths, error: MarsError) -> MarsError {
    match crate::platform::fs::safe_remove(&paths.new) {
        Ok(()) => error,
        Err(cleanup_error) => MarsError::Internal(format!(
            "plugin tree publication failed: {error}; staging cleanup also failed: {cleanup_error}"
        )),
    }
}

fn write_sentinel(out_dir: &Path, sentinel: &PluginSentinel) -> Result<(), MarsError> {
    let mut bytes = serde_json::to_vec_pretty(sentinel).map_err(|error| {
        MarsError::Internal(format!("failed to serialize plugin sentinel: {error}"))
    })?;
    bytes.push(b'\n');
    crate::fs::atomic_write(&out_dir.join(SENTINEL_FILE), &bytes)
}

fn path_exists(path: &Path) -> Result<bool, MarsError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("inspect plugin path", path, source)),
    }
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
