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

/// Prepare one authored package exactly once without materializing it.
pub(crate) fn prepare_rooted(
    source_name: &crate::types::SourceName,
    rooted: crate::resolve::RootedSourceRef,
    effective_config: &crate::config::EffectiveConfig,
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
    let package_name = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.reading.fields.as_ref())
        .map(|package| package.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(source_name.as_ref());
    report_diagnostics(
        &prepared,
        &rooted.authored_root,
        &format!(
            "package `{package_name}` uses the pre-v1 format; run `mars migrate` in its repository"
        ),
        diag,
    );

    prepared_rooted(rooted, Arc::new(prepared), diag)
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
                prepared_manifest.reading.fields.as_ref(),
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
    let prepared = mars_schema::prepare::prepare(&source, context, dialect.map(schema_dialect));
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    let legacy_warning = match context {
        PrepareContext::AuthoredLocal => {
            "authored `.mars-src` content uses the pre-v1 format; run `mars migrate`".to_string()
        }
        PrepareContext::Package => package_legacy_warning(&prepared, root),
    };
    report_diagnostics(&prepared, root, &legacy_warning, diag);
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
    let legacy_warning = match context {
        PrepareContext::AuthoredLocal => {
            "authored `.mars-src` content uses the pre-v1 format; run `mars migrate`".to_string()
        }
        PrepareContext::Package => package_legacy_warning(&prepared, root),
    };
    report_diagnostics(&prepared, root, &legacy_warning, diag);
    Ok(prepared)
}

pub(crate) fn report_diagnostics(
    prepared: &Prepared,
    authored_root: &Path,
    legacy_warning: &str,
    diag: &mut DiagnosticCollector,
) {
    let mut warn_legacy = false;
    for (relative, diagnostics) in prepared.all_diagnostics() {
        let context = format!("source `{}`", authored_root.join(relative).display());
        for diagnostic in diagnostics {
            if diagnostic.code == "legacy-package" {
                warn_legacy = true;
                continue;
            }
            if prepared.classification == Classification::MarsLegacy
                && diagnostic.code == "field.removed"
                && diagnostic.message.starts_with("removed legacy key")
            {
                continue;
            }
            diag.schema(diagnostic, &context, false);
        }
    }
    if warn_legacy {
        diag.warn_once("legacy-package", legacy_warning);
    }
}

fn package_legacy_warning(prepared: &Prepared, root: &Path) -> String {
    let name = prepared
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.reading.fields.as_ref())
        .map(|package| package.name.as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| root.file_name().and_then(|name| name.to_str()))
        .unwrap_or("unknown");
    format!("package `{name}` uses the pre-v1 format; run `mars migrate` in its repository")
}
