//! Filesystem adapter for crate-owned package preparation.
//!
//! The schema crate owns classification, definition selection, normalization and
//! strict reading. This module supplies lazy authored-tree IO and maps the pure
//! result onto binary diagnostics and materialization.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mars_schema::dialect::Dialect;
use mars_schema::inspect::{Hint, Kind};
use mars_schema::normalize::{NormalizeRequest, Selection, SourceInfo, normalize};
use mars_schema::prepare::{Classification, PrepareContext, Prepared};
use mars_schema::tree::{FileTree, SubTree, TreeSource};

use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;

/// Lazy, package-confined filesystem implementation of `TreeSource`.
///
/// IO/policy failures are latched because the host-independent trait deliberately
/// has no error type. Callers must take the error immediately after `prepare`.
pub(crate) struct DiskTreeSource {
    root: PathBuf,
    canonical_root: PathBuf,
    error: RefCell<Option<MarsError>>,
}

impl DiskTreeSource {
    pub(crate) fn new(root: &Path) -> Result<Self, MarsError> {
        let canonical_root = dunce::canonicalize(root).map_err(|source| MarsError::Io {
            operation: "resolve authored package root".to_string(),
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            canonical_root,
            error: RefCell::new(None),
        })
    }

    pub(crate) fn take_error(&self) -> Option<MarsError> {
        self.error.borrow_mut().take()
    }

    fn path(&self, relative: &str) -> Option<PathBuf> {
        if !valid_relative(relative) {
            self.latch(MarsError::InvalidRequest {
                message: format!("invalid package-relative path `{relative}`"),
            });
            return None;
        }
        Some(if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        })
    }

    fn confined(&self, logical: &Path) -> Option<PathBuf> {
        match dunce::canonicalize(logical) {
            Ok(path) if path.starts_with(&self.canonical_root) => Some(path),
            Ok(_) => {
                self.latch(MarsError::InvalidRequest {
                    message: format!(
                        "authored package entry escapes package root: {}",
                        logical.display()
                    ),
                });
                None
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                self.latch(MarsError::Io {
                    operation: "resolve authored package entry".to_string(),
                    path: logical.to_path_buf(),
                    source,
                });
                None
            }
        }
    }

    fn latch(&self, error: MarsError) {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(error);
        }
    }
}

impl TreeSource for DiskTreeSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let logical = self.path(path)?;
        let actual = self.confined(&logical)?;
        match fs::metadata(&actual) {
            Ok(metadata) if metadata.is_file() => match fs::read(&actual) {
                Ok(bytes) => Some(bytes),
                Err(source) => {
                    self.latch(MarsError::Io {
                        operation: "read authored package file".to_string(),
                        path: logical,
                        source,
                    });
                    None
                }
            },
            Ok(_) => None,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                self.latch(MarsError::Io {
                    operation: "inspect authored package file".to_string(),
                    path: logical,
                    source,
                });
                None
            }
        }
    }

    fn is_file(&self, path: &str) -> bool {
        self.path(path)
            .and_then(|path| self.confined(&path))
            .and_then(|path| fs::metadata(path).ok())
            .is_some_and(|metadata| metadata.is_file())
    }

    fn is_dir(&self, path: &str) -> bool {
        self.path(path)
            .and_then(|path| self.confined(&path))
            .and_then(|path| fs::metadata(path).ok())
            .is_some_and(|metadata| metadata.is_dir())
    }

    fn children(&self, dir: &str) -> Vec<String> {
        let Some(logical) = self.path(dir) else {
            return Vec::new();
        };
        let Some(actual) = self.confined(&logical) else {
            return Vec::new();
        };
        // A file has no children, as in the in-memory tree the WASM build reads: discovery asks
        // for the children of every `skills/` entry, and a foreign tree's `skills/` can hold
        // files (`__init__.py`, `README.md`) beside the skill directories.
        if !fs::metadata(&actual).is_ok_and(|metadata| metadata.is_dir()) {
            return Vec::new();
        }
        let entries = match fs::read_dir(&actual) {
            Ok(entries) => entries,
            Err(source) => {
                self.latch(MarsError::Io {
                    operation: "list authored package directory".to_string(),
                    path: logical,
                    source,
                });
                return Vec::new();
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => {
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        self.latch(MarsError::InvalidRequest {
                            message: format!(
                                "authored package entry name is not UTF-8 below {}",
                                logical.display()
                            ),
                        });
                        continue;
                    };
                    let child = if dir.is_empty() {
                        name
                    } else {
                        format!("{dir}/{name}")
                    };
                    // Resolve now so an escaping symlink cannot masquerade as an
                    // absent definition or payload entry.
                    if self
                        .path(&child)
                        .and_then(|path| self.confined(&path))
                        .is_some()
                    {
                        children.push(child);
                    }
                }
                Err(source) => self.latch(MarsError::Io {
                    operation: "read authored package directory entry".to_string(),
                    path: logical.clone(),
                    source,
                }),
            }
        }
        children.sort();
        children
    }
}

/// The files of one package, read by whoever prepares or inspects it. A git source brings the tree
/// its provider read from the content directory, and the package is the part of it below `prefix`;
/// a path source has none and is read from disk, lazily and confined to the package root.
pub(crate) enum PackageFiles {
    Fetched { tree: Arc<FileTree>, prefix: String },
    Live(DiskTreeSource),
}

impl PackageFiles {
    /// The package rooted at `root`, a directory at or below `checkout_root`, which `tree` holds.
    pub(crate) fn open(
        tree: Option<&Arc<FileTree>>,
        checkout_root: &Path,
        root: &Path,
    ) -> Result<Self, MarsError> {
        let Some(tree) = tree else {
            return Ok(Self::Live(DiskTreeSource::new(root)?));
        };
        let below = root
            .strip_prefix(checkout_root)
            .map_err(|_| MarsError::InvalidRequest {
                message: format!(
                    "package root {} is not inside its checkout {}",
                    root.display(),
                    checkout_root.display()
                ),
            })?;
        let prefix = below
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok(Self::Fetched {
            tree: Arc::clone(tree),
            prefix,
        })
    }

    /// The error a disk read latched; a fetched tree reads without one.
    pub(crate) fn take_error(&self) -> Option<MarsError> {
        match self {
            Self::Fetched { .. } => None,
            Self::Live(disk) => disk.take_error(),
        }
    }
}

impl TreeSource for PackageFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Self::Fetched { tree, prefix } => SubTree::new(&**tree, prefix.as_str()).read(path),
            Self::Live(disk) => disk.read(path),
        }
    }

    fn is_file(&self, path: &str) -> bool {
        match self {
            Self::Fetched { tree, prefix } => SubTree::new(&**tree, prefix.as_str()).is_file(path),
            Self::Live(disk) => disk.is_file(path),
        }
    }

    fn is_dir(&self, path: &str) -> bool {
        match self {
            Self::Fetched { tree, prefix } => SubTree::new(&**tree, prefix.as_str()).is_dir(path),
            Self::Live(disk) => disk.is_dir(path),
        }
    }

    fn children(&self, dir: &str) -> Vec<String> {
        match self {
            Self::Fetched { tree, prefix } => SubTree::new(&**tree, prefix.as_str()).children(dir),
            Self::Live(disk) => disk.children(dir),
        }
    }
}

fn valid_relative(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && (path.is_empty()
            || !path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | "..")))
}

pub(crate) struct PreparedRootedSource {
    pub rooted: crate::resolve::RootedSourceRef,
    pub prepared: Arc<Prepared>,
    pub manifest: Option<crate::config::Manifest>,
    pub hook_surface: crate::staging::HookSurfaceState,
}

pub(crate) fn prepare_rooted(
    source_name: &crate::types::SourceName,
    rooted: crate::resolve::RootedSourceRef,
    effective_config: &crate::config::EffectiveConfig,
    version: Option<&semver::Version>,
    upgrading: bool,
    diag: &mut DiagnosticCollector,
) -> Result<PreparedRootedSource, MarsError> {
    let dependency = effective_config.dependencies.get(source_name);
    let source = PackageFiles::open(
        rooted.tree.as_ref(),
        &rooted.checkout_root,
        &rooted.authored_root,
    )?;
    let request = NormalizeRequest {
        tree: &source,
        source: SourceInfo {
            name: Some(source_name.to_string()),
            ..SourceInfo::default()
        },
        hint: Hint {
            dialect: dependency.and_then(|entry| entry.dialect),
            plugin: rooted.plugin.as_ref().map(|plugin| plugin.hint.clone()),
            ..Hint::default()
        },
        selection: Selection::default(),
    };
    let mut normalized = normalize(&request);
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    for (path, diagnostics) in &normalized.diagnostics {
        for diagnostic in diagnostics {
            diag.schema(diagnostic, &format!("{source_name}: {path}"), false);
        }
    }
    if normalized.kind == Kind::Marketplace {
        return Err(match &rooted.plugin {
            // The plugin was chosen from its marketplace; what is wrong is the plugin itself,
            // which the crate refuses by reporting the root as a marketplace.
            Some(plugin) => MarsError::InvalidRequest {
                message: format!(
                    "`{source_name}`, plugin `{}`: {}",
                    plugin.name(),
                    normalized
                        .diagnostics
                        .values()
                        .flatten()
                        .find(|diagnostic| diagnostic.is_error())
                        .map_or("it cannot be imported", |refusal| refusal.message.as_str())
                ),
            },
            None => marketplace_choice_error(source_name, &source, &request.hint),
        });
    }
    let [package] = normalized.packages.as_mut_slice() else {
        return Err(MarsError::InvalidRequest {
            message: format!("`{source_name}` did not normalize to exactly one package"),
        });
    };
    let prepared = package.prepared.clone();
    let package_name = dependency_name(&prepared, source_name);
    // Only a git source is a tree the consumer cannot edit; a path source or an override is
    // theirs, and `mars upgrade` would leave it unchanged.
    let subject = match dependency.map(|entry| &entry.spec) {
        Some(crate::config::SourceSpec::Git(_)) => {
            Subject::Dependency(package_name, version, upgrading)
        }
        _ => Subject::Package(package_name),
    };
    report_diagnostics(
        &prepared,
        &|relative| format!("{package_name}: {relative}"),
        &subject,
        false,
        diag,
    );

    prepared_rooted(rooted, Arc::new(prepared), diag)
}

/// How diagnostics name a dependency: by its manifest, else by its source name; never by where
/// the cache keeps its archive.
pub(crate) fn dependency_name<'a>(
    prepared: &'a Prepared,
    source_name: &'a crate::types::SourceName,
) -> &'a str {
    prepared
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.reading.package.fields.as_ref())
        .map(|package| package.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(source_name.as_ref())
}

/// A marketplace dependency installs one plugin, and the lock records which: the dependency
/// must say which it wants.
fn marketplace_choice_error(
    source_name: &crate::types::SourceName,
    source: &PackageFiles,
    hint: &Hint,
) -> MarsError {
    let inspection = mars_schema::inspect::inspect(source, hint);
    MarsError::InvalidRequest {
        message: format!(
            "`{source_name}` is a marketplace with these plugins:\n{}\nname the one to install with `--plugin <name>` on `mars add`, or `plugin = \"<name>\"` on its dependency entry",
            crate::resolve::plugin_list(&inspection.plugins)
        ),
    }
}

pub(crate) fn prepared_rooted(
    rooted: crate::resolve::RootedSourceRef,
    prepared: Arc<Prepared>,
    diag: &mut DiagnosticCollector,
) -> Result<PreparedRootedSource, MarsError> {
    let manifest = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| std::str::from_utf8(&manifest.normalized_bytes).ok())
        .map(|content| {
            let prepared_manifest = prepared
                .manifest
                .as_ref()
                .expect("prepared manifest bytes came from a prepared manifest");
            crate::config::parse_prepared_manifest(
                content,
                &rooted.authored_root.join("mars.toml"),
                &prepared_manifest.reading,
            )
        })
        .transpose()?
        .flatten();
    let hook_surface = crate::staging::detect_unreadable_hook_surface(&rooted.authored_root, diag)?;
    Ok(PreparedRootedSource {
        rooted,
        prepared,
        manifest,
        hook_surface,
    })
}

pub(crate) fn prepare_authored(
    root: &Path,
    context: PrepareContext,
    dialect: Option<Dialect>,
    diag: &mut DiagnosticCollector,
) -> Result<Prepared, MarsError> {
    let source = DiskTreeSource::new(root)?;
    let prepared = mars_schema::prepare::prepare(&source, context, dialect);
    reported(root, source, prepared, false, FileFindings::Print, diag)
}

fn reported(
    root: &Path,
    source: DiskTreeSource,
    prepared: Prepared,
    own_files: bool,
    findings: FileFindings,
    diag: &mut DiagnosticCollector,
) -> Result<Prepared, MarsError> {
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    // A package carries its manifest; authored content has none to name it by.
    let package_name = prepared
        .manifest
        .is_some()
        .then(|| authored_package_name(&prepared, root));
    let subject = match &package_name {
        Some(name) => Subject::Package(name),
        None => Subject::AuthoredSource,
    };
    match findings {
        FileFindings::Print => {
            report_diagnostics(&prepared, &source_label(root), &subject, own_files, diag);
        }
        FileFindings::Leave => report_translation(&prepared, &subject, diag),
    }
    Ok(prepared)
}

/// Whether preparing a package prints what `prepare` found in each file.
#[derive(Clone, Copy)]
pub(crate) enum FileFindings {
    Print,
    /// The caller prints them itself, from `mars_schema::check::check`.
    Leave,
}

/// Prepare a directory that is a package root or a consumer project, with the `mars.toml`
/// bytes a publish lens reads: the prepared manifest, or a consumer project's own
/// configuration, which preparation does not touch.
pub(crate) fn prepare_project(
    root: &Path,
    findings: FileFindings,
    diag: &mut DiagnosticCollector,
) -> Result<(Prepared, Option<Vec<u8>>), MarsError> {
    let source = DiskTreeSource::new(root)?;
    let mut prepared = mars_schema::prepare::prepare(&source, PrepareContext::Project, None);
    // `mars check` judges the package's own layout; a sync judges installed trees.
    mars_schema::links::package(&mut prepared);
    let prepared = reported(root, source, prepared, true, findings, diag)?;
    let manifest = match &prepared.manifest {
        Some(manifest) => Some(manifest.normalized_bytes.clone()),
        None => match fs::read(root.join("mars.toml")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "read consumer configuration".to_string(),
                    path: root.join("mars.toml"),
                    source,
                });
            }
        },
    };
    Ok((prepared, manifest))
}

pub(crate) enum Subject<'a> {
    Package(&'a str),
    /// A git package a consumer depends on, at the version the sync resolved. The flag is set
    /// when the command is `mars upgrade` for this package, which has already tried to move it.
    Dependency(&'a str, Option<&'a semver::Version>, bool),
    AuthoredSource,
}

/// `label` names a package-relative path in a diagnostic. A dependency is named by its
/// package (`meridian-base: skills/poc/SKILL.md`), never by where the cache keeps its
/// archive; a project's own sources keep their real path.
///
/// What translating changed in each file (`Prepared::changes`) is not repeated on every sync: a
/// translated package gets a summary line with the count of files. Except the ones that lost a
/// restriction, which are printed every time with the file they are in. `own_files` is a package's own checkout
/// (`mars check`), where the author can fix the file: it also prints each ignored execution
/// field and each quoted frontmatter value.
pub(crate) fn report_diagnostics(
    prepared: &Prepared,
    label: &dyn Fn(&str) -> String,
    subject: &Subject<'_>,
    own_files: bool,
    diag: &mut DiagnosticCollector,
) {
    report_files(prepared, label, subject, own_files, diag);
    report_translation(prepared, subject, diag);
}

/// What `prepare` found in each file, and the changes the author can act on.
fn report_files(
    prepared: &Prepared,
    label: &dyn Fn(&str) -> String,
    subject: &Subject<'_>,
    own_files: bool,
    diag: &mut DiagnosticCollector,
) {
    for (relative, diagnostics) in prepared.all_diagnostics() {
        let context = label(relative);
        for diagnostic in diagnostics {
            match subject {
                // The consumer cannot edit the package, so the schema's wording for its author
                // is no use to them.
                Subject::Dependency(name, version, upgrading)
                    if diagnostic.code == "manifest.schema-missing" =>
                {
                    let mut for_consumer = diagnostic.clone();
                    let resolved = version
                        .map_or_else(String::new, |version| format!(", resolved to {version},"));
                    let way_out = if *upgrading {
                        "`mars upgrade` cannot move it past its `version` constraint: widen that constraint in mars.toml"
                    } else {
                        "run `mars upgrade {name}` for a newer version (widen its `version` constraint if `mars upgrade` keeps this version)"
                    };
                    for_consumer.message = format!(
                        "this dependency{resolved} was published before schema 1. Mars 0.34.0 and later do not read it: {}, or stay on a Mars before 0.34.0",
                        way_out.replace("{name}", name)
                    );
                    diag.schema(&for_consumer, &context, false);
                }
                _ => diag.schema(diagnostic, &context, false),
            }
        }
    }
    for (relative, lost) in prepared.lost_restrictions() {
        diag.schema(lost, &label(relative), false);
    }
    if own_files {
        for (relative, ignored) in prepared
            .ignored_execution_fields()
            .chain(prepared.repaired_values())
        {
            diag.schema(ignored, &label(relative), false);
        }
    }
}

/// One line for a translated package: how many files were changed.
fn report_translation(prepared: &Prepared, subject: &Subject<'_>, diag: &mut DiagnosticCollector) {
    let files = prepared.changed_files();
    if files == 0 {
        return;
    }
    if let Classification::Foreign(choice) = &prepared.classification {
        let name = match subject {
            Subject::Package(name) | Subject::Dependency(name, ..) => format!("package `{name}`"),
            Subject::AuthoredSource => "authored `.mars-src` content".to_string(),
        };
        let count = format!("{files} file{}", if files == 1 { "" } else { "s" });
        diag.info(
            "package-translated",
            format!("{name} is read as {}: {count} translated", choice.dialect),
        );
    }
}

pub(crate) fn source_label(root: &Path) -> impl Fn(&str) -> String + '_ {
    move |relative| format!("source `{}`", root.join(relative).display())
}

fn authored_package_name(prepared: &Prepared, root: &Path) -> String {
    prepared
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.reading.package.fields.as_ref())
        .map(|package| package.name.as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| root.file_name().and_then(|name| name.to_str()))
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
pub(crate) fn prepare_tree_for_test(name: &str, root: &std::path::Path) -> Arc<Prepared> {
    let config = crate::config::EffectiveConfig {
        dependencies: indexmap::IndexMap::new(),
        settings: crate::config::Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };
    let rooted = crate::resolve::RootedSourceRef {
        checkout_root: root.to_path_buf(),
        tree: None,
        authored_root: root.to_path_buf(),
        package_root: root.to_path_buf(),
        plugin: None,
    };
    let prepared = prepare_rooted(
        &name.into(),
        rooted,
        &config,
        None,
        false,
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    prepared.prepared
}
