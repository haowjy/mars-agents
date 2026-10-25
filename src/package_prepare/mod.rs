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
    _diag: &mut DiagnosticCollector,
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
        ManifestClassification::MarsLegacy => Ok(DefinitionPreparation::Legacy),
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

/// Classify, normalize, and strictly read one rooted package in caller-owned scratch space.
/// Relative dependency paths have already been resolved from `rooted`'s authored origin.
pub(crate) fn prepare_rooted(
    source_name: &crate::types::SourceName,
    rooted: crate::resolve::RootedSourceRef,
    effective_config: &crate::config::EffectiveConfig,
    staging_root: Option<&Path>,
    diag: &mut DiagnosticCollector,
) -> Result<crate::staging::StagedRootedSource, MarsError> {
    let dep = effective_config.dependencies.get(source_name);
    let preparation = classify(
        source_name.as_ref(),
        &rooted.package_root,
        dep.and_then(|entry| entry.dialect),
        diag,
    )?;
    let Some(staging_root) = staging_root else {
        if preparation == DefinitionPreparation::Legacy {
            return Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package `{source_name}` uses the pre-v1 format, but this operation does not materialize packages; run `mars migrate` in that package first"
                ),
            });
        }
        return Ok(crate::staging::StagedRootedSource {
            rooted,
            hook_surface: crate::staging::HookSurfaceState::Readable,
        });
    };
    let empty_renames = crate::types::RenameMap::new();
    let renames = dep.map(|entry| &entry.rename).unwrap_or(&empty_renames);
    crate::staging::stage_rooted_source(
        source_name,
        rooted,
        crate::staging::RootedStageOptions { preparation },
        &effective_config.skills,
        renames,
        staging_root,
        diag,
    )
}
