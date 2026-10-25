use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::PluginHarness;
use crate::error::MarsError;
use crate::plugin_metadata::{
    CLAUDE_CATALOG, CURSOR_CATALOG, OPENCODE_PACKAGE, PluginSentinel, SENTINEL_FILE,
};

use super::{configured_output_path, plugin_inputs_hash};

/// Offline state of a configured plugin build and the repo-root files it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOutputState {
    pub directory: PluginDirectoryState,
    pub inputs: Option<PluginInputsState>,
    pub version: Option<PluginVersionState>,
    pub root_files: Vec<PluginRootFileState>,
    pub root_directories: BTreeSet<String>,
}

impl PluginOutputState {
    pub fn root_file(&self, path: &Path) -> Option<&PluginRootFileState> {
        self.root_files.iter().find(|state| state.path() == path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginDirectoryState {
    Missing {
        path: PathBuf,
    },
    Unowned {
        path: PathBuf,
        reason: String,
    },
    Unchanged {
        path: PathBuf,
    },
    Changed {
        path: PathBuf,
        changed_paths: Vec<PathBuf>,
    },
}

impl PluginDirectoryState {
    /// A user-facing finding, or `None` when the directory is unchanged.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Missing { path } => Some(format!(
                "plugin output {} is missing; it will be built on the next release",
                path.display()
            )),
            Self::Unowned { path, reason } => Some(format!(
                "plugin output {} has no valid Mars sentinel: {reason}",
                path.display()
            )),
            Self::Unchanged { .. } => None,
            Self::Changed {
                path,
                changed_paths,
            } => Some(format!(
                "plugin output {} has hand edits or missing files ({}); the next build will rewrite it",
                path.display(),
                display_paths(changed_paths)
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginInputsState {
    Current,
    Stale { output: PathBuf },
}

impl PluginInputsState {
    /// A user-facing finding, or `None` when the input fingerprint is current.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Current => None,
            Self::Stale { output } => Some(format!(
                "plugin output {} is older than its authored sources; rebuild it with `mars build plugin`",
                output.display()
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginVersionState {
    Current,
    Skipped {
        output: PathBuf,
        built: String,
        package: String,
    },
}

impl PluginVersionState {
    /// A user-facing finding, or `None` when the package and output versions agree.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Current => None,
            Self::Skipped {
                output,
                built,
                package,
            } => Some(format!(
                "plugin output {} is version {built}, but mars.toml is version {package}; a release may have used `mars version --no-plugins`",
                output.display()
            )),
        }
    }
}

/// Ownership state for one package-relative generated root file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRootFileState {
    Absent {
        path: PathBuf,
        recorded_hash: Option<String>,
    },
    RecordedUnchanged {
        path: PathBuf,
        recorded_hash: String,
    },
    RecordedChanged {
        path: PathBuf,
        recorded_hash: String,
    },
    Unrecorded {
        path: PathBuf,
    },
}

impl PluginRootFileState {
    pub fn path(&self) -> &Path {
        match self {
            Self::Absent { path, .. }
            | Self::RecordedUnchanged { path, .. }
            | Self::RecordedChanged { path, .. }
            | Self::Unrecorded { path } => path,
        }
    }

    /// Whether Mars may replace this path with `bytes`.
    ///
    /// An absent path and an unchanged recorded path are owned. A changed or
    /// unrecorded path is accepted only when publishing would be byte-identical.
    pub(crate) fn allows_write(
        &self,
        package_root: &Path,
        bytes: &[u8],
    ) -> Result<bool, MarsError> {
        let current = root_file_hash(package_root, self.path())?;
        let desired_hash = crate::hash::hash_bytes(bytes);
        Ok(match current {
            RootPathState::Missing => true,
            RootPathState::Regular(hash) => {
                hash == desired_hash
                    || self
                        .recorded_hash()
                        .is_some_and(|recorded| hash == recorded)
            }
            RootPathState::NonRegular => false,
        })
    }

    /// Whether Mars may delete this path when a harness is removed.
    pub(crate) fn allows_delete(&self, package_root: &Path) -> Result<bool, MarsError> {
        let current = root_file_hash(package_root, self.path())?;
        Ok(match current {
            RootPathState::Missing => true,
            RootPathState::Regular(hash) => self
                .recorded_hash()
                .is_some_and(|recorded| hash == recorded),
            RootPathState::NonRegular => false,
        })
    }

    pub(crate) fn was_recorded(&self) -> bool {
        self.recorded_hash().is_some()
    }

    fn recorded_hash(&self) -> Option<&str> {
        match self {
            Self::Absent { recorded_hash, .. } => recorded_hash.as_deref(),
            Self::RecordedUnchanged { recorded_hash, .. }
            | Self::RecordedChanged { recorded_hash, .. } => Some(recorded_hash),
            Self::Unrecorded { .. } => None,
        }
    }

    /// A user-facing finding, or `None` when the file is absent or unchanged.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Absent { .. } | Self::RecordedUnchanged { .. } => None,
            Self::RecordedChanged { path, .. } => Some(root_file_conflict_message(
                path,
                "differs from the hash in the current Mars sentinel",
            )),
            Self::Unrecorded { path } => Some(format!(
                "generated root file {} is an unmanaged user file, but the {} harness needs that path for generated plugin metadata",
                path.display(),
                root_file_harness(path)
            )),
        }
    }
}

/// Inspect a configured plugin output without resolving dependencies.
pub fn inspect(package_root: &Path) -> Result<PluginOutputState, MarsError> {
    let package_root = dunce::canonicalize(package_root).map_err(|source| MarsError::Io {
        operation: "resolve plugin package root".to_string(),
        path: package_root.to_path_buf(),
        source,
    })?;
    let manifest = crate::config::load(&package_root)?;
    let plugin = manifest
        .plugin
        .as_ref()
        .ok_or_else(|| MarsError::InvalidRequest {
            message: "inspecting plugin output needs a [plugin] table in mars.toml".to_string(),
        })?;
    let package = manifest
        .package
        .as_ref()
        .expect("plugin config validation requires a package table");
    let output = configured_output_path(&package_root, plugin)?;
    inspect_output(
        &package_root,
        &output,
        &package.version,
        expected_root_paths(&plugin.harnesses),
    )
}

pub(super) fn expected_root_paths(harnesses: &[PluginHarness]) -> BTreeSet<&'static str> {
    let mut paths = BTreeSet::new();
    if harnesses.iter().any(|harness| {
        matches!(
            harness,
            PluginHarness::Claude | PluginHarness::Codex | PluginHarness::Grok
        )
    }) {
        paths.insert(CLAUDE_CATALOG);
    }
    if harnesses.contains(&PluginHarness::Cursor) {
        paths.insert(CURSOR_CATALOG);
    }
    if harnesses.contains(&PluginHarness::OpenCode) {
        paths.insert(OPENCODE_PACKAGE);
    }
    paths
}

pub(super) fn inspect_output(
    package_root: &Path,
    output: &Path,
    package_version: &str,
    expected_root_files: BTreeSet<&'static str>,
) -> Result<PluginOutputState, MarsError> {
    let sentinel = match fs::symlink_metadata(output) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(io_error("inspect plugin output", output, source));
        }
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return without_sentinel(
                package_root,
                expected_root_files,
                PluginDirectoryState::Unowned {
                    path: output.to_path_buf(),
                    reason: "the configured output is not a regular directory".to_string(),
                },
            );
        }
        Ok(_) => match crate::plugin_metadata::read(output) {
            Ok(sentinel) => Some(sentinel),
            Err(error) => {
                return without_sentinel(
                    package_root,
                    expected_root_files,
                    PluginDirectoryState::Unowned {
                        path: output.to_path_buf(),
                        reason: error.to_string(),
                    },
                );
            }
        },
    };

    let Some(sentinel) = sentinel else {
        return without_sentinel(
            package_root,
            expected_root_files,
            PluginDirectoryState::Missing {
                path: output.to_path_buf(),
            },
        );
    };

    let changed_paths = changed_output_paths(output, &sentinel)?;
    let directory = if changed_paths.is_empty() {
        PluginDirectoryState::Unchanged {
            path: output.to_path_buf(),
        }
    } else {
        PluginDirectoryState::Changed {
            path: output.to_path_buf(),
            changed_paths,
        }
    };
    let inputs = if plugin_inputs_hash(package_root)? == sentinel.inputs_hash {
        PluginInputsState::Current
    } else {
        PluginInputsState::Stale {
            output: output.to_path_buf(),
        }
    };
    let version = if package_version == sentinel.version {
        PluginVersionState::Current
    } else {
        PluginVersionState::Skipped {
            output: output.to_path_buf(),
            built: sentinel.version.clone(),
            package: package_version.to_string(),
        }
    };
    let root_files = inspect_root_files(
        package_root,
        expected_root_files,
        Some(&sentinel.root_files),
    )?;

    Ok(PluginOutputState {
        directory,
        inputs: Some(inputs),
        version: Some(version),
        root_files,
        root_directories: sentinel.root_directories,
    })
}

fn without_sentinel(
    package_root: &Path,
    expected_root_files: BTreeSet<&'static str>,
    directory: PluginDirectoryState,
) -> Result<PluginOutputState, MarsError> {
    Ok(PluginOutputState {
        directory,
        inputs: None,
        version: None,
        root_files: inspect_root_files(package_root, expected_root_files, None)?,
        root_directories: BTreeSet::new(),
    })
}

fn inspect_root_files(
    package_root: &Path,
    expected: BTreeSet<&'static str>,
    recorded: Option<&BTreeMap<String, String>>,
) -> Result<Vec<PluginRootFileState>, MarsError> {
    let mut paths: BTreeSet<String> = expected.into_iter().map(str::to_string).collect();
    if let Some(recorded) = recorded {
        paths.extend(recorded.keys().cloned());
    }

    paths
        .into_iter()
        .map(|path| {
            let relative = PathBuf::from(&path);
            if !relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err(MarsError::InvalidRequest {
                    message: format!("invalid generated root-file path in Mars sentinel: {path}"),
                });
            }
            let state = root_file_hash(package_root, &relative)?;
            Ok(match state {
                RootPathState::Missing => PluginRootFileState::Absent {
                    path: relative,
                    recorded_hash: recorded.and_then(|files| files.get(&path)).cloned(),
                },
                RootPathState::Regular(hash)
                    if recorded.and_then(|files| files.get(&path)) == Some(&hash) =>
                {
                    PluginRootFileState::RecordedUnchanged {
                        path: relative,
                        recorded_hash: hash,
                    }
                }
                RootPathState::Regular(_) | RootPathState::NonRegular
                    if recorded.is_some_and(|files| files.contains_key(&path)) =>
                {
                    PluginRootFileState::RecordedChanged {
                        path: relative,
                        recorded_hash: recorded
                            .and_then(|files| files.get(&path))
                            .expect("recorded path has a hash")
                            .clone(),
                    }
                }
                RootPathState::Regular(_) | RootPathState::NonRegular => {
                    PluginRootFileState::Unrecorded { path: relative }
                }
            })
        })
        .collect()
}

enum RootPathState {
    Missing,
    Regular(String),
    NonRegular,
}

fn root_file_hash(package_root: &Path, relative: &Path) -> Result<RootPathState, MarsError> {
    let mut current = package_root.to_path_buf();
    let component_count = relative.components().count();
    for (index, component) in relative.components().enumerate() {
        current.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RootPathState::Missing);
            }
            Err(source) => return Err(io_error("inspect generated root file", &current, source)),
        };
        if metadata.file_type().is_symlink() {
            return Ok(RootPathState::NonRegular);
        }
        if index + 1 < component_count {
            if !metadata.is_dir() {
                return Ok(RootPathState::NonRegular);
            }
        } else if metadata.is_file() {
            let bytes = fs::read(&current)
                .map_err(|source| io_error("read generated root file", &current, source))?;
            return Ok(RootPathState::Regular(crate::hash::hash_bytes(&bytes)));
        } else {
            return Ok(RootPathState::NonRegular);
        }
    }
    Ok(RootPathState::NonRegular)
}

fn changed_output_paths(
    output: &Path,
    sentinel: &PluginSentinel,
) -> Result<Vec<PathBuf>, MarsError> {
    let mut actual = BTreeMap::new();
    collect_output_entries(output, output, &mut actual)?;

    let paths: BTreeSet<String> = sentinel
        .files
        .keys()
        .chain(actual.keys())
        .cloned()
        .collect();
    Ok(paths
        .into_iter()
        .filter(|path| {
            !matches!(
                (actual.get(path), sentinel.files.get(path)),
                (Some(Some(actual)), Some(recorded)) if actual == recorded
            )
        })
        .map(PathBuf::from)
        .collect())
}

fn collect_output_entries(
    root: &Path,
    directory: &Path,
    entries: &mut BTreeMap<String, Option<String>>,
) -> Result<(), MarsError> {
    let read_dir = fs::read_dir(directory)
        .map_err(|source| io_error("read plugin output", directory, source))?;
    for entry in read_dir {
        let entry = entry.map_err(|source| io_error("read plugin output", directory, source))?;
        let path = entry.path();
        let relative = crate::hash::portable_relative(root, &path).ok_or_else(|| {
            MarsError::InvalidRequest {
                message: format!(
                    "generated plugin path must be valid Unicode: {}",
                    path.display()
                ),
            }
        })?;
        if relative == SENTINEL_FILE {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|source| io_error("inspect plugin output entry", &path, source))?;
        if file_type.is_dir() {
            collect_output_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path)
                .map_err(|source| io_error("read plugin output file", &path, source))?;
            entries.insert(relative, Some(crate::hash::hash_bytes(&bytes)));
        } else {
            entries.insert(relative, None);
        }
    }
    Ok(())
}

fn root_file_conflict_message(path: &Path, reason: &str) -> String {
    format!(
        "generated root file {} {reason}. It may be stale generated output after a partial checkout, a merge that mixed builds, or an interrupted build whose inputs changed. Run `git restore -- {}` or delete it before rebuilding",
        path.display(),
        path.display()
    )
}

fn root_file_harness(path: &Path) -> &'static str {
    if path == Path::new(CLAUDE_CATALOG) {
        "Claude/Codex/Grok"
    } else if path == Path::new(CURSOR_CATALOG) {
        "Cursor"
    } else if path == Path::new(OPENCODE_PACKAGE) {
        "OpenCode"
    } else {
        "requested"
    }
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn io_error(operation: &str, path: &Path, source: std::io::Error) -> MarsError {
    MarsError::Io {
        operation: operation.to_string(),
        path: path.to_path_buf(),
        source,
    }
}
