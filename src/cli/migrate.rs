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
            return Err(MarsError::InvalidRequest { message });
        }
        ManifestClassification::NoManifest => unreachable!("manifest bytes were provided"),
    }

    let mut pending = Vec::new();
    push_if_changed(
        root,
        manifest_path,
        &manifest_text,
        upgrade_manifest(&manifest_text).map_err(upgrade_error)?,
        &mut pending,
    );
    for (path, kind) in definition_files(root)? {
        let text = std::fs::read_to_string(&path)?;
        let upgraded = upgrade_definition(kind, &text).map_err(upgrade_error)?;
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

fn report(check: bool, pending: Vec<PendingWrite>, json: bool) -> Result<i32, MarsError> {
    let files = pending
        .iter()
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    if json {
        output::print_json(&MigrateReport {
            check,
            changed: !files.is_empty(),
            files,
        });
    } else if files.is_empty() {
        output::print_info("definition format is already schema v1");
    } else {
        let verb = if check { "would rewrite" } else { "rewrote" };
        for file in &files {
            output::print_info(&format!("{verb} {file}"));
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
    let mut files = Vec::new();
    for (base, authored_root) in [
        (root.join("agents"), false),
        (root.join("skills"), false),
        (root.join(".mars-src"), true),
    ] {
        if !base.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&base).follow_links(false) {
            let entry = entry.map_err(|error| MarsError::InvalidRequest {
                message: format!("failed to walk {}: {error}", base.display()),
            })?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let kind = if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                Some(DefinitionKind::Skill)
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("md")
                && (!authored_root
                    || path
                        .components()
                        .any(|component| component.as_os_str() == "agents"))
            {
                Some(DefinitionKind::Agent)
            } else {
                None
            };
            if let Some(kind) = kind {
                files.push((path.to_path_buf(), kind));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files.dedup_by(|left, right| left.0 == right.0);
    Ok(files)
}

fn upgrade_error(error: mars_schema::upgrade::UpgradeError) -> MarsError {
    MarsError::InvalidRequest {
        message: error.to_string(),
    }
}
