//! Package-format dispatch before any lift or definition discovery.

use std::path::Path;

use mars_schema::package::{ManifestClassification, classify_manifest};

use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::error::MarsError;
use crate::staging::DefinitionPreparation;

pub(crate) fn classify(
    package_name: &str,
    package_root: &Path,
    declared_dialect: Option<Dialect>,
    diag: &mut DiagnosticCollector,
) -> Result<DefinitionPreparation, MarsError> {
    let manifest_path = package_root.join("mars.toml");
    let manifest = match std::fs::read(&manifest_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(MarsError::Io {
                operation: "read package manifest for classification".to_string(),
                path: manifest_path,
                source,
            });
        }
    };
    match classify_manifest(manifest.as_deref()) {
        ManifestClassification::Schema1 => Ok(DefinitionPreparation::Schema1),
        ManifestClassification::MarsLegacy => {
            diag.warn_once(
                "legacy-package",
                format!("package `{package_name}` uses the pre-v1 format; run `mars migrate`"),
            );
            Ok(DefinitionPreparation::Legacy)
        }
        ManifestClassification::SchemaNewer { schema } => Err(MarsError::Source {
            source_name: package_name.to_string(),
            message: format!(
                "package `{package_name}` declares schema {schema}, which is newer than supported schema 1"
            ),
        }),
        ManifestClassification::Invalid { message } => Err(MarsError::Source {
            source_name: package_name.to_string(),
            message,
        }),
        ManifestClassification::NoManifest => Ok(DefinitionPreparation::Foreign(Dialect::resolve(
            declared_dialect,
            package_root,
        ))),
    }
}
