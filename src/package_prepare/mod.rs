//! Filesystem adapter for crate-owned package preparation.
//!
//! The schema crate owns classification, definition selection, normalization and
//! strict reading. This module supplies lazy authored-tree IO and maps the pure
//! result onto binary diagnostics and materialization.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use mars_schema::dialect::Dialect;
use mars_schema::prepare::{Classification, PrepareContext, Prepared};
use mars_schema::tree::TreeSource;

use crate::diagnostic::{DiagnosticCollector, Owner};
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
            // A path below a file (`agents/x.md/.mars-plugin.json`) is absent, as in `FileTree`:
            // callers probe every prefix of a path they were given.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }
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

fn valid_relative(path: &str) -> bool {
    !path.starts_with('/')
        && !path.contains('\\')
        && (path.is_empty()
            || !path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | "..")))
}

/// What preparing a resolved dependency's files found, framed for its consumer: who can edit the
/// files (a git source cannot) decides how a finding is worded.
pub(crate) fn report_dependency(
    effective_config: &crate::config::EffectiveConfig,
    source_name: &crate::types::SourceName,
    version: Option<&semver::Version>,
    upgrading: bool,
    prepared: &Prepared,
    normalized: &std::collections::BTreeMap<String, Vec<mars_schema::diagnostic::SchemaDiagnostic>>,
    diag: &mut DiagnosticCollector,
) {
    let dependency = effective_config.dependencies.get(source_name);
    for (path, diagnostics) in normalized {
        for diagnostic in diagnostics {
            diag.schema(
                diagnostic,
                &format!("{source_name}: {path}"),
                Owner::Dependency(source_name.as_str()),
            );
        }
    }
    // The installer reads a dependency's `[models]` and nothing else of its `mars.toml`, so the
    // alias that names a retired harness is only reported here.
    if let Some(table) = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| std::str::from_utf8(&manifest.normalized_bytes).ok())
        .and_then(|text| text.parse::<toml::Table>().ok())
    {
        for diagnostic in mars_schema::retired::model_harness_diagnostics(&table) {
            diag.schema(
                &diagnostic,
                &format!("{source_name}: mars.toml"),
                Owner::Dependency(source_name.as_str()),
            );
        }
    }
    let package_name = dependency_name(prepared, source_name);
    // Only a git source is a tree the consumer cannot edit; a path source or an override is
    // theirs, and `mars upgrade` would leave it unchanged. A source the consumer's own config
    // does not name came in through another package, which is git.
    let subject = match dependency.map(|entry| &entry.spec) {
        Some(crate::config::SourceSpec::Path(_)) => Subject::Package(package_name),
        _ => Subject::Dependency(package_name, source_name.as_str(), version, upgrading),
    };
    report_diagnostics(
        prepared,
        &|relative| format!("{package_name}: {relative}"),
        &subject,
        false,
        diag,
    );
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
    /// A git package a consumer depends on: its name, the source name the lock records its items
    /// under, and the version the sync resolved. The flag is set when the command is `mars
    /// upgrade` for this package, which has already tried to move it.
    Dependency(&'a str, &'a str, Option<&'a semver::Version>, bool),
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
    let owner = match subject {
        Subject::Dependency(_, source, ..) => Owner::Dependency(source),
        Subject::Package(_) | Subject::AuthoredSource => Owner::Own,
    };
    for (relative, diagnostics) in prepared.all_diagnostics() {
        let context = label(relative);
        for diagnostic in diagnostics {
            match subject {
                // The consumer cannot edit the package, so the schema's wording for its author
                // is no use to them.
                Subject::Dependency(name, _, version, upgrading)
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
                    diag.schema(&for_consumer, &context, owner);
                }
                _ => diag.schema(diagnostic, &context, owner),
            }
        }
    }
    for (relative, lost) in prepared.lost_restrictions() {
        diag.schema(lost, &label(relative), owner);
    }
    if own_files {
        for (relative, ignored) in prepared
            .ignored_execution_fields()
            .chain(prepared.repaired_values())
        {
            diag.schema(ignored, &label(relative), owner);
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
pub(crate) fn prepare_tree_for_test(
    name: &str,
    root: &std::path::Path,
) -> std::sync::Arc<Prepared> {
    use mars_schema::inspect::Hint;
    use mars_schema::normalize::{NormalizeRequest, Selection, SourceInfo, normalize};
    let source = DiskTreeSource::new(root).unwrap();
    let request = NormalizeRequest {
        tree: &source,
        source: SourceInfo {
            name: Some(name.to_string()),
            ..SourceInfo::default()
        },
        hint: Hint::default(),
        selection: Selection::default(),
    };
    let mut normalized = normalize(&request);
    std::sync::Arc::new(normalized.packages.remove(0).prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin lists `./agent.md`; discovery then asks whether `agent.md/.mars-plugin.json`
    /// holds a generated-tree marker. A file has no children: that is absent, not an I/O error.
    #[test]
    fn a_path_below_a_file_is_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("agent.md"), "body").unwrap();
        let source = DiskTreeSource::new(dir.path()).unwrap();

        assert_eq!(source.read("agent.md/.mars-plugin.json"), None);
        assert!(!source.is_file("agent.md/.mars-plugin.json"));
        assert!(!source.is_dir("agent.md/x"));
        assert!(source.take_error().is_none());
    }
}
