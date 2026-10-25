use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MarsError;

pub const SENTINEL_FILE: &str = ".mars-plugin.json";
const SENTINEL_SCHEMA_VERSION: u32 = 1;

/// Build metadata recorded in the generated plugin tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentinelInfo {
    pub plugin_name: String,
    pub version: String,
    pub inputs_hash: String,
}

/// Persisted ownership and staleness metadata for a generated plugin tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSentinel {
    pub schema_version: u32,
    pub mars_version: String,
    pub plugin_name: String,
    pub version: String,
    pub inputs_hash: String,
    pub tree_hash: String,
}

/// Replace a Mars-owned plugin tree as one crash-recoverable operation.
///
/// `write` receives an empty sibling staging directory. The existing output is
/// not moved until `write`, hashing, and the sentinel write have all succeeded.
pub fn replace_owned_tree(
    out_dir: &Path,
    sentinel: &SentinelInfo,
    write: impl FnOnce(&Path) -> Result<(), MarsError>,
) -> Result<(), MarsError> {
    validate_sentinel_info(sentinel)?;
    let paths = OutputPaths::new(out_dir)?;
    let _lock = crate::fs::FileLock::acquire(&paths.lock)?;

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
        write(&paths.new)?;
        validate_staging_tree(&paths.new)?;

        let tree_hash = compute_tree_hash(&paths.new)?;
        let stored = PluginSentinel {
            schema_version: SENTINEL_SCHEMA_VERSION,
            mars_version: env!("CARGO_PKG_VERSION").to_string(),
            plugin_name: sentinel.plugin_name.clone(),
            version: sentinel.version.clone(),
            inputs_hash: sentinel.inputs_hash.clone(),
            tree_hash,
        };
        write_sentinel(&paths.new, &stored)
    })();

    if let Err(build_error) = staged {
        return match crate::platform::fs::safe_remove(&paths.new) {
            Ok(()) => Err(build_error),
            Err(cleanup_error) => Err(MarsError::Internal(format!(
                "plugin tree build failed: {build_error}; staging cleanup also failed: {cleanup_error}"
            ))),
        };
    }

    publish_staged_tree(&paths)
}

/// Read and validate a generated plugin tree's sentinel.
pub fn read_sentinel(out_dir: &Path) -> Result<PluginSentinel, MarsError> {
    let metadata = fs::symlink_metadata(out_dir)
        .map_err(|source| io_error("inspect plugin tree", out_dir, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(format!(
            "plugin tree must be a directory, not {}",
            out_dir.display()
        )));
    }
    read_sentinel_file(&out_dir.join(SENTINEL_FILE))
}

/// Recompute the deterministic hash of every regular output file except the sentinel.
pub fn compute_tree_hash(out_dir: &Path) -> Result<String, MarsError> {
    let metadata = fs::symlink_metadata(out_dir)
        .map_err(|source| io_error("inspect plugin tree", out_dir, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(format!(
            "plugin tree must be a directory, not {}",
            out_dir.display()
        )));
    }

    let mut entries = Vec::new();
    collect_tree_hashes(out_dir, out_dir, &mut entries)?;
    Ok(crate::hash::hash_file_manifest(entries))
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
    crate::platform::fs::safe_remove(&paths.new)?;

    let old = classify_tree(&paths.old)?;
    if matches!(old, TreeState::Missing) {
        return Ok(());
    }
    let output = classify_tree(&paths.out)?;

    match (old, output) {
        // A valid live tree proves the second rename completed. The rollback
        // directory may be only partly removed, including loss of its sentinel.
        (_, TreeState::Owned) => crate::platform::fs::safe_remove(&paths.old),
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

    Ok(match read_sentinel(path) {
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

fn publish_staged_tree(paths: &OutputPaths) -> Result<(), MarsError> {
    match publish_staged_tree_inner(paths) {
        Ok(()) => Ok(()),
        Err(error) => Err(clean_up_staging_after_error(paths, error)),
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

fn read_sentinel_file(path: &Path) -> Result<PluginSentinel, MarsError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| io_error("inspect plugin sentinel", path, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid(format!(
            "plugin sentinel must be a regular file: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|source| io_error("read plugin sentinel", path, source))?;
    let sentinel: PluginSentinel = serde_json::from_slice(&bytes).map_err(|error| {
        invalid(format!(
            "invalid plugin sentinel {}: {error}",
            path.display()
        ))
    })?;
    validate_stored_sentinel(path, &sentinel)?;
    Ok(sentinel)
}

fn validate_stored_sentinel(path: &Path, sentinel: &PluginSentinel) -> Result<(), MarsError> {
    if sentinel.schema_version != SENTINEL_SCHEMA_VERSION {
        return Err(invalid(format!(
            "unsupported plugin sentinel schema {} in {}",
            sentinel.schema_version,
            path.display()
        )));
    }
    if sentinel.mars_version.trim().is_empty()
        || sentinel.plugin_name.trim().is_empty()
        || sentinel.version.trim().is_empty()
        || sentinel.inputs_hash.trim().is_empty()
        || !is_sha256(&sentinel.tree_hash)
    {
        return Err(invalid(format!(
            "plugin sentinel has invalid metadata: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_sentinel_info(sentinel: &SentinelInfo) -> Result<(), MarsError> {
    if sentinel.plugin_name.trim().is_empty() {
        return Err(invalid("plugin name cannot be empty"));
    }
    if sentinel.version.trim().is_empty() {
        return Err(invalid("plugin version cannot be empty"));
    }
    if sentinel.inputs_hash.trim().is_empty() {
        return Err(invalid("plugin input hash cannot be empty"));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn path_exists(path: &Path) -> Result<bool, MarsError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("inspect plugin path", path, source)),
    }
}

fn collect_tree_hashes(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(String, String)>,
) -> Result<(), MarsError> {
    let children =
        fs::read_dir(current).map_err(|source| io_error("read plugin tree", current, source))?;
    for child in children {
        let child = child.map_err(|source| io_error("read plugin tree", current, source))?;
        let path = child.path();
        let file_type = child
            .file_type()
            .map_err(|source| io_error("inspect plugin tree entry", &path, source))?;
        let relative = path
            .strip_prefix(root)
            .expect("plugin tree traversal remains below its root");

        if relative == Path::new(SENTINEL_FILE) {
            continue;
        }
        if file_type.is_symlink() {
            return Err(invalid(format!(
                "plugin output cannot contain symlinks: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_tree_hashes(root, &path, entries)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(invalid(format!(
                "plugin output contains an unsupported filesystem entry: {}",
                path.display()
            )));
        }

        let relative = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| {
                        invalid(format!(
                            "plugin output path must be valid Unicode: {}",
                            path.display()
                        ))
                    })
                    .map(str::to_owned)
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let contents =
            fs::read(&path).map_err(|source| io_error("read plugin output file", &path, source))?;
        entries.push((relative, crate::hash::hash_bytes(&contents)));
    }
    Ok(())
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
