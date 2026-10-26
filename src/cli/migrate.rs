//! `mars migrate` — upgrade authored package definitions to schema v1.

use std::path::{Path, PathBuf};

use mars_schema::package::{ManifestClassification, classify_manifest};
use mars_schema::upgrade::{DefinitionKind, UpgradeResult, upgrade_definition, upgrade_manifest};
use serde::Serialize;

use crate::error::MarsError;

use super::output;

#[derive(Debug, clap::Args)]
pub struct MigrateArgs {
    /// Report files that need migration without writing them.
    #[arg(long)]
    pub check: bool,
}

#[derive(Serialize)]
struct MigrateReport {
    check: bool,
    changed: bool,
    files: Vec<String>,
    fully_rewritten_files: Vec<String>,
}

struct PendingWrite {
    path: PathBuf,
    relative: String,
    result: UpgradeResult,
}

pub fn run(args: &MigrateArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let root = &ctx.project_root;
    let manifest_path = root.join("mars.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)?;
    let consumer = is_consumer_manifest(&manifest_text);
    match classify_manifest(Some(manifest_text.as_bytes())) {
        ManifestClassification::Schema1 => return report(args.check, Vec::new(), json),
        ManifestClassification::MarsLegacy => {}
        ManifestClassification::SchemaNewer { schema } => {
            return Err(MarsError::InvalidRequest {
                message: format!(
                    "package declares schema {schema}, which is newer than supported schema 1"
                ),
            });
        }
        ManifestClassification::Invalid { message } => {
            if consumer {
                return migrate_consumer(args, root, json);
            }
            return Err(MarsError::InvalidRequest { message });
        }
        ManifestClassification::NoManifest => unreachable!("manifest bytes were provided"),
    }

    let prepared = tempfile::tempdir()?;
    let mut prepare_diag = crate::diagnostic::DiagnosticCollector::new();
    crate::staging::stage_prepared_package(
        root,
        prepared.path(),
        crate::staging::DefinitionPreparation::Legacy,
        &[],
        &mut prepare_diag,
    )?;
    prepare_diag.fail_on_errors()?;

    let mut pending = Vec::new();
    let mut manifest_result = upgrade_manifest(&manifest_text).map_err(upgrade_error)?;
    manifest_result.text = std::fs::read_to_string(prepared.path().join("mars.toml"))?;
    push_if_changed(
        root,
        manifest_path,
        &manifest_text,
        manifest_result,
        &mut pending,
    );
    for (path, kind) in definition_files(root)? {
        let text = std::fs::read_to_string(&path)?;
        let mut upgraded = upgrade_definition(kind, &text).map_err(upgrade_error)?;
        let relative = path
            .strip_prefix(root)
            .expect("discovered definitions are below the package root");
        upgraded.text = std::fs::read_to_string(prepared.path().join(relative))?;
        push_if_changed(root, path, &text, upgraded, &mut pending);
    }
    pending.sort_by(|left, right| left.relative.cmp(&right.relative));

    if !args.check {
        for file in &pending {
            crate::platform::fs::atomic_write_file(&file.path, file.result.text.as_bytes())?;
        }
    }
    for file in &pending {
        for diagnostic in &file.result.diagnostics {
            if !json {
                output::print_warn(&format!("{}: {}", file.relative, diagnostic.message));
            }
        }
    }
    report(args.check, pending, json)
}

fn migrate_consumer(args: &MigrateArgs, root: &Path, json: bool) -> Result<i32, MarsError> {
    let authored_root = root.join(".mars-src");
    let prepared = if authored_root.is_dir() {
        let prepared = tempfile::tempdir()?;
        let mut prepare_diag = crate::diagnostic::DiagnosticCollector::new();
        crate::staging::stage_prepared_package(
            &authored_root,
            prepared.path(),
            crate::staging::DefinitionPreparation::Legacy,
            &[],
            &mut prepare_diag,
        )?;
        prepare_diag.fail_on_errors()?;
        Some(prepared)
    } else {
        None
    };
    let mut pending = Vec::new();
    for (path, kind) in definition_files(&authored_root)? {
        let text = std::fs::read_to_string(&path)?;
        let mut upgraded = upgrade_definition(kind, &text).map_err(upgrade_error)?;
        let relative = path
            .strip_prefix(&authored_root)
            .expect("discovered definitions are below .mars-src");
        upgraded.text = std::fs::read_to_string(
            prepared
                .as_ref()
                .expect("discovered .mars-src definitions require a prepared tree")
                .path()
                .join(relative),
        )?;
        push_if_changed(root, path, &text, upgraded, &mut pending);
    }
    pending.sort_by(|left, right| left.relative.cmp(&right.relative));
    if !args.check {
        for file in &pending {
            crate::platform::fs::atomic_write_file(&file.path, file.result.text.as_bytes())?;
        }
    }
    if pending.is_empty() && !json {
        output::print_info(&format!(
            "nothing to migrate: {} is consumer configuration, not a package; dependencies migrate in their own repositories",
            root.display()
        ));
        return Ok(0);
    }
    report(args.check, pending, json)
}

fn is_consumer_manifest(text: &str) -> bool {
    text.parse::<toml::Value>()
        .is_ok_and(|manifest| manifest.get("package").is_none())
}

fn report(check: bool, pending: Vec<PendingWrite>, json: bool) -> Result<i32, MarsError> {
    let files = pending
        .iter()
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    let fully_rewritten_files = pending
        .iter()
        .filter(|file| file.result.fully_rewritten)
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    if json {
        output::print_json(&MigrateReport {
            check,
            changed: !files.is_empty(),
            files,
            fully_rewritten_files,
        });
    } else if files.is_empty() {
        output::print_info("definition format is already schema v1");
    } else {
        for file in &pending {
            let verb = match (check, file.result.fully_rewritten) {
                (true, true) => "would fully rewrite",
                (true, false) => "would edit",
                (false, true) => "fully rewrote",
                (false, false) => "edited",
            };
            output::print_info(&format!("{verb} {}", file.relative));
        }
    }
    Ok(i32::from(check && !pending.is_empty()))
}

fn push_if_changed(
    root: &Path,
    path: PathBuf,
    original: &str,
    result: UpgradeResult,
    pending: &mut Vec<PendingWrite>,
) {
    if !result.changed_from(original) {
        return;
    }
    let relative = path
        .strip_prefix(root)
        .expect("migration paths are below the project root")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    pending.push(PendingWrite {
        path,
        relative,
        result,
    });
}

fn definition_files(root: &Path) -> Result<Vec<(PathBuf, DefinitionKind)>, MarsError> {
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    crate::discover::discover_definition_files(root, None, &mut diag)
}

fn upgrade_error(error: mars_schema::upgrade::UpgradeError) -> MarsError {
    MarsError::InvalidRequest {
        message: error.to_string(),
    }
}
