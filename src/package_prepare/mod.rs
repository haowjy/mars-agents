//! Filesystem adapter for crate-owned package preparation.
//!
//! The schema crate owns classification, definition selection, normalization and
//! strict reading. This module supplies lazy authored-tree IO and maps the pure
//! result onto binary diagnostics and materialization.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mars_schema::prepare::{Classification, PrepareContext, Prepared};
use mars_schema::tree::TreeSource;

use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::error::MarsError;

fn schema_dialect(dialect: Dialect) -> mars_schema::dialect::Dialect {
    match dialect {
        Dialect::Claude => mars_schema::dialect::Dialect::Claude,
        Dialect::Codex => mars_schema::dialect::Dialect::Codex,
        Dialect::OpenCode => mars_schema::dialect::Dialect::OpenCode,
        Dialect::Cursor => mars_schema::dialect::Dialect::Cursor,
        Dialect::MarsNative => mars_schema::dialect::Dialect::MarsNative,
    }
}

/// Lazy, package-confined filesystem implementation of `TreeSource`.
///
/// IO/policy failures are latched because the host-independent trait deliberately
/// has no error type. Callers must take the error immediately after `prepare`.
pub(crate) struct DiskTreeSource {
    root: PathBuf,
    canonical_root: PathBuf,
    hide_manifest: bool,
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
            hide_manifest: false,
            error: RefCell::new(None),
        })
    }

    pub(crate) fn without_manifest(mut self) -> Self {
        self.hide_manifest = true;
        self
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
        if self.hide_manifest && path == "mars.toml" {
            return None;
        }
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
        if self.hide_manifest && path == "mars.toml" {
            return false;
        }
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

pub(crate) struct PreparedRootedSource {
    pub rooted: crate::resolve::RootedSourceRef,
    pub prepared: Arc<Prepared>,
    pub manifest: Option<crate::config::Manifest>,
    pub hook_surface: crate::staging::HookSurfaceState,
}

/// Prepare one authored package exactly once, then optionally materialize its
/// normalized projection for existing filesystem consumers.
pub(crate) fn prepare_rooted(
    source_name: &crate::types::SourceName,
    rooted: crate::resolve::RootedSourceRef,
    effective_config: &crate::config::EffectiveConfig,
    staging_root: Option<&Path>,
    diag: &mut DiagnosticCollector,
) -> Result<PreparedRootedSource, MarsError> {
    let dependency = effective_config.dependencies.get(source_name);
    let source = DiskTreeSource::new(&rooted.authored_root)?;
    let prepared = mars_schema::prepare::prepare(
        &source,
        PrepareContext::Package,
        dependency
            .and_then(|entry| entry.dialect)
            .map(schema_dialect),
    );
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    report_diagnostics(&prepared, &rooted.authored_root, diag);

    let manifest = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| std::str::from_utf8(&manifest.normalized_bytes).ok())
        .map(|content| {
            crate::config::parse_prepared_manifest(content, &rooted.authored_root.join("mars.toml"))
        })
        .transpose()?
        .flatten();

    if staging_root.is_none() && matches!(prepared.classification, Classification::MarsLegacy) {
        return Err(MarsError::Source {
            source_name: source_name.to_string(),
            message: format!(
                "package `{source_name}` uses the pre-v1 format, but this operation does not materialize packages; run `mars migrate` in that package first"
            ),
        });
    }

    let prepared = Arc::new(prepared);
    let (rooted, hook_surface) = if let Some(staging_root) = staging_root {
        let empty_renames = crate::types::RenameMap::new();
        let renames = dependency
            .map(|entry| &entry.rename)
            .unwrap_or(&empty_renames);
        crate::staging::materialize_prepared_rooted(
            source_name,
            rooted,
            Arc::clone(&prepared),
            &effective_config.skills,
            renames,
            staging_root,
            diag,
        )?
    } else {
        (rooted, crate::staging::HookSurfaceState::Readable)
    };

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
    let prepared = mars_schema::prepare::prepare(&source, context, dialect.map(schema_dialect));
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    report_diagnostics(&prepared, root, diag);
    Ok(prepared)
}

pub(crate) fn prepare_authored_without_manifest(
    root: &Path,
    context: PrepareContext,
    dialect: Option<Dialect>,
    diag: &mut DiagnosticCollector,
) -> Result<Prepared, MarsError> {
    let source = DiskTreeSource::new(root)?.without_manifest();
    let prepared = mars_schema::prepare::prepare(&source, context, dialect.map(schema_dialect));
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    report_diagnostics(&prepared, root, diag);
    Ok(prepared)
}

pub(crate) fn report_diagnostics(
    prepared: &Prepared,
    authored_root: &Path,
    diag: &mut DiagnosticCollector,
) {
    for (relative, diagnostics) in &prepared.diagnostics {
        let context = format!("source `{}`", authored_root.join(relative).display());
        for diagnostic in diagnostics {
            diag.schema(diagnostic, &context, false);
        }
    }
}
