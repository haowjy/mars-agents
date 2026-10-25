use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::config::PluginHarness;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::plugin_metadata::SENTINEL_FILE;

use super::PluginBuildDestination;
use super::inspect::{self, PluginRootFileState};
use super::metadata::PackageMetadata;

pub(super) struct RootFilePlan {
    pub(super) root_files: BTreeMap<String, String>,
    pub(super) root_directories: BTreeSet<String>,
    package_root: PathBuf,
    writes: BTreeMap<String, Vec<u8>>,
    deletes: BTreeSet<String>,
    directory_deletes: BTreeSet<String>,
    ownership: BTreeMap<String, PluginRootFileState>,
}

impl RootFilePlan {
    /// Package-relative root files this plan will write or delete.
    pub(super) fn touched_paths(&self) -> Vec<PathBuf> {
        self.writes
            .keys()
            .chain(self.deletes.iter())
            .map(PathBuf::from)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn deleted_paths(&self) -> Vec<PathBuf> {
        self.deletes.iter().map(PathBuf::from).collect()
    }
}

pub(super) fn plan(
    package_root: &Path,
    out_dir: &Path,
    staging: &Path,
    destination: &PluginBuildDestination,
    metadata: &PackageMetadata,
    harnesses: &[PluginHarness],
    diag: &mut DiagnosticCollector,
) -> Result<RootFilePlan, MarsError> {
    let package_root = dunce::canonicalize(package_root).map_err(|source| MarsError::Io {
        operation: "resolve plugin package root".to_string(),
        path: package_root.to_path_buf(),
        source,
    })?;

    if matches!(destination, PluginBuildDestination::Out(_)) {
        return Ok(RootFilePlan {
            root_files: BTreeMap::new(),
            root_directories: BTreeSet::new(),
            package_root,
            writes: BTreeMap::new(),
            deletes: BTreeSet::new(),
            directory_deletes: BTreeSet::new(),
            ownership: BTreeMap::new(),
        });
    }

    let output_relative =
        crate::hash::portable_relative(&package_root, out_dir).ok_or_else(|| {
            MarsError::InvalidRequest {
                message: format!(
                    "configured plugin output must be a portable path inside the package: {}",
                    out_dir.display()
                ),
            }
        })?;

    let expected = inspect::expected_root_paths(harnesses);
    let mut writes = BTreeMap::new();
    if expected.contains(".claude-plugin/marketplace.json") {
        insert_catalog(
            &mut writes,
            ".claude-plugin",
            &format!("./{output_relative}/claude"),
            &metadata.marketplace,
            metadata,
        )?;
    }
    if expected.contains(".cursor-plugin/marketplace.json") {
        insert_catalog(
            &mut writes,
            ".cursor-plugin",
            &format!("{output_relative}/cursor"),
            &metadata.marketplace,
            metadata,
        )?;
    }
    if expected.contains("package.json") {
        writes.insert(
            "package.json".to_string(),
            opencode_package_bytes(metadata, &output_relative)?,
        );
    }

    if metadata.author.is_none()
        && expected
            .iter()
            .any(|path| path.ends_with("marketplace.json"))
    {
        diag.warn(
            "plugin-catalog-owner-fallback",
            format!(
                "[package].authors is empty; generated marketplace owner falls back to plugin name `{}`",
                metadata.name
            ),
        );
    }

    let state =
        inspect::inspect_output(&package_root, out_dir, &metadata.version, expected.clone())?;
    let mut deletes = BTreeSet::new();
    let mut directory_deletes = BTreeSet::new();
    let mut ownership = BTreeMap::new();

    for root_file in &state.root_files {
        let key = portable_path(root_file.path())?;
        if let Some(bytes) = writes.get(&key) {
            if !root_file.allows_write(&package_root, bytes)? {
                return Err(root_file_refusal(root_file, "replace"));
            }
            ownership.insert(key, root_file.clone());
        } else if root_file.was_recorded() {
            if !root_file.allows_delete(&package_root)? {
                return Err(root_file_refusal(root_file, "delete"));
            }
            deletes.insert(key.clone());
            if let Some(directory) = catalog_directory(&key)
                && state.root_directories.contains(directory)
            {
                directory_deletes.insert(directory.to_string());
            }
            ownership.insert(key, root_file.clone());
        }
    }

    let mut root_directories = BTreeSet::new();
    for path in writes.keys() {
        let Some(directory) = catalog_directory(path) else {
            continue;
        };
        if state.root_directories.contains(directory)
            || !path_exists(&package_root.join(directory))?
        {
            root_directories.insert(directory.to_string());
        }
    }

    let touched: BTreeSet<String> = writes.keys().chain(deletes.iter()).cloned().collect();
    check_gitignored(&package_root, out_dir, staging, &touched)?;

    let root_files = writes
        .iter()
        .map(|(path, bytes)| (path.clone(), crate::hash::hash_bytes(bytes)))
        .collect();
    Ok(RootFilePlan {
        root_files,
        root_directories,
        package_root,
        writes,
        deletes,
        directory_deletes,
        ownership,
    })
}

pub(super) fn apply(plan: &RootFilePlan) -> Result<(), MarsError> {
    for (relative, bytes) in &plan.writes {
        let ownership = plan.ownership.get(relative).ok_or_else(|| {
            MarsError::Internal(format!(
                "generated root-file plan omitted ownership state for {relative}"
            ))
        })?;
        if !ownership.allows_write(&plan.package_root, bytes)? {
            return Err(root_file_refusal(ownership, "replace"));
        }
        crate::fs::atomic_write_if_changed(&plan.package_root.join(relative), bytes)?;
    }
    for relative in &plan.deletes {
        let ownership = plan.ownership.get(relative).ok_or_else(|| {
            MarsError::Internal(format!(
                "generated root-file plan omitted ownership state for {relative}"
            ))
        })?;
        if !ownership.allows_delete(&plan.package_root)? {
            return Err(root_file_refusal(ownership, "delete"));
        }
        let path = plan.package_root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(&path).map_err(|source| MarsError::Io {
                    operation: "delete retired generated root file".to_string(),
                    path,
                    source,
                })?;
            }
            Ok(_) => {
                return Err(MarsError::InvalidRequest {
                    message: format!(
                        "refusing to delete generated root file {} because it is no longer a regular file",
                        path.display()
                    ),
                });
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "inspect retired generated root file".to_string(),
                    path,
                    source,
                });
            }
        }
    }
    for relative in &plan.directory_deletes {
        remove_empty_catalog_directory(&plan.package_root, relative)?;
    }
    Ok(())
}

fn remove_empty_catalog_directory(package_root: &Path, relative: &str) -> Result<(), MarsError> {
    let directory = package_root.join(relative);
    match fs::remove_dir(&directory) {
        Ok(()) => Ok(()),
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(MarsError::Io {
            operation: "remove empty retired plugin catalog directory".to_string(),
            path: directory,
            source,
        }),
    }
}

fn catalog_directory(path: &str) -> Option<&'static str> {
    match path {
        crate::plugin_metadata::CLAUDE_CATALOG => Some(crate::plugin_metadata::CLAUDE_CATALOG_DIR),
        crate::plugin_metadata::CURSOR_CATALOG => Some(crate::plugin_metadata::CURSOR_CATALOG_DIR),
        _ => None,
    }
}

fn path_exists(path: &Path) -> Result<bool, MarsError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(MarsError::Io {
            operation: "inspect generated root directory".to_string(),
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[derive(Serialize)]
struct Catalog<'a> {
    name: &'a str,
    owner: CatalogOwner<'a>,
    plugins: [CatalogPlugin<'a>; 1],
}

#[derive(Serialize)]
struct CatalogOwner<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
}

#[derive(Serialize)]
struct CatalogPlugin<'a> {
    name: &'a str,
    source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

fn insert_catalog(
    writes: &mut BTreeMap<String, Vec<u8>>,
    directory: &str,
    source: &str,
    marketplace: &str,
    metadata: &PackageMetadata,
) -> Result<(), MarsError> {
    let owner = metadata.author.as_ref();
    let catalog = Catalog {
        name: marketplace,
        owner: CatalogOwner {
            name: owner.map_or(metadata.name.as_str(), |author| author.name.as_str()),
            email: owner.and_then(|author| author.email.as_deref()),
        },
        plugins: [CatalogPlugin {
            name: &metadata.name,
            source,
            description: metadata.description.as_deref(),
        }],
    };
    writes.insert(
        format!("{directory}/marketplace.json"),
        pretty_json(&catalog, "plugin marketplace")?,
    );
    Ok(())
}

fn opencode_package_bytes(
    metadata: &PackageMetadata,
    output_relative: &str,
) -> Result<Vec<u8>, MarsError> {
    #[derive(Serialize)]
    struct RootPackage<'a> {
        name: &'a str,
        version: &'a str,
        main: String,
        files: [String; 1],
    }

    let opencode = format!("{output_relative}/opencode");
    pretty_json(
        &RootPackage {
            name: &metadata.name,
            version: &metadata.version,
            main: format!("{opencode}/index.mjs"),
            files: [opencode],
        },
        "root OpenCode package",
    )
}

fn pretty_json(value: &impl Serialize, description: &str) -> Result<Vec<u8>, MarsError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        MarsError::Internal(format!("failed to serialize {description}: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn root_file_refusal(state: &PluginRootFileState, action: &str) -> MarsError {
    let detail = state.message().unwrap_or_else(|| {
        format!(
            "generated root file {} is not owned by Mars",
            state.path().display()
        )
    });
    MarsError::InvalidRequest {
        message: format!("refusing to {action} {}: {detail}", state.path().display()),
    }
}

fn check_gitignored(
    package_root: &Path,
    out_dir: &Path,
    staging: &Path,
    root_files: &BTreeSet<String>,
) -> Result<(), MarsError> {
    let Some(git_root) = git_worktree_root(package_root)? else {
        return Ok(());
    };

    let mut candidates = BTreeSet::from([out_dir.to_path_buf(), out_dir.join(SENTINEL_FILE)]);
    for relative in crate::hash::strict_file_hashes(staging, &[])?.keys() {
        candidates.insert(out_dir.join(relative));
    }
    for relative in root_files {
        candidates.insert(package_root.join(relative));
    }

    let relative_paths: Vec<String> = candidates
        .into_iter()
        .filter_map(|path| path.strip_prefix(&git_root).ok().map(Path::to_path_buf))
        .map(|path| portable_path(&path))
        .collect::<Result<_, _>>()?;
    if relative_paths.is_empty() {
        return Ok(());
    }

    let mut command = Command::new("git");
    crate::platform::process::remove_git_local_env(&mut command);
    let mut child = command
        .current_dir(&git_root)
        .args(["check-ignore", "-v", "--no-index", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| git_error("git check-ignore --stdin", source.to_string()))?;
    let mut input = Vec::new();
    for path in &relative_paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take().expect("piped git stdin");
    // Git may report every path. Feed stdin concurrently with wait_with_output,
    // which drains stdout and stderr, so a large ignored tree cannot deadlock
    // when both pipe buffers fill.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|source| git_error("git check-ignore --stdin", source.to_string()))?;
    writer
        .join()
        .map_err(|_| MarsError::Internal("git check-ignore stdin writer panicked".to_string()))?
        .map_err(|source| git_error("git check-ignore --stdin", source.to_string()))?;
    match output.status.code() {
        Some(1) => Ok(()),
        Some(0) => {
            let fields: Vec<&[u8]> = output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|field| !field.is_empty())
                .collect();
            if fields.is_empty() || !fields.len().is_multiple_of(4) {
                return Err(git_error(
                    "git check-ignore --stdin",
                    "unexpected verbose output while checking generated plugin paths".to_string(),
                ));
            }
            let ignored = fields.chunks_exact(4).find_map(|record| {
                let [source, line, pattern, path] = record else {
                    unreachable!("chunks_exact returns four fields")
                };
                (!pattern.starts_with(b"!")).then_some((*source, *line, *pattern, *path))
            });
            let Some((source, line, pattern, path)) = ignored else {
                return Ok(());
            };
            Err(MarsError::InvalidRequest {
                message: format!(
                    "generated plugin path `{}` is gitignored by {}:{} pattern `{}`; remove the ignore rule before building",
                    String::from_utf8_lossy(path),
                    String::from_utf8_lossy(source),
                    String::from_utf8_lossy(line),
                    String::from_utf8_lossy(pattern),
                ),
            })
        }
        _ => Err(git_error(
            "git check-ignore --stdin",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )),
    }
}

fn git_worktree_root(package_root: &Path) -> Result<Option<PathBuf>, MarsError> {
    let mut command = Command::new("git");
    crate::platform::process::remove_git_local_env(&mut command);
    let output = match command
        .current_dir(package_root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(output) => output,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(git_error(
                "git rev-parse --show-toplevel",
                source.to_string(),
            ));
        }
    };
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        if message
            .to_ascii_lowercase()
            .contains("not a git repository")
        {
            return Ok(None);
        }
        return Err(git_error(
            "git rev-parse --show-toplevel",
            message.trim().to_string(),
        ));
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    dunce::canonicalize(&path)
        .map(Some)
        .map_err(|source| MarsError::Io {
            operation: "resolve git worktree root".to_string(),
            path,
            source,
        })
}

fn portable_path(path: &Path) -> Result<String, MarsError> {
    path.components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| MarsError::InvalidRequest {
                    message: format!(
                        "generated plugin path must be valid Unicode: {}",
                        path.display()
                    ),
                })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

fn git_error(command: &str, message: String) -> MarsError {
    MarsError::GitCli {
        command: command.to_string(),
        message,
    }
}
