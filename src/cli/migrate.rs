//! `mars migrate` — apply crate-owned authored-file changes for schema v1.

use std::path::{Path, PathBuf};

use mars_schema::package::{ManifestClassification, classify_manifest};
use mars_schema::prepare::{ChangeRecord, PrepareContext, Prepared};
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
    /// What the migration removed or could not carry over, by file.
    dropped: Vec<DroppedKey>,
}

#[derive(Serialize)]
struct DroppedKey {
    file: String,
    code: String,
    message: String,
}

struct PendingWrite {
    path: PathBuf,
    relative: String,
    bytes: Vec<u8>,
    fully_rewritten: bool,
}

pub fn run(args: &MigrateArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let root = &ctx.project_root;
    let manifest = std::fs::read(root.join("mars.toml"))?;
    let consumer = classify_manifest(Some(&manifest)) == ManifestClassification::ConsumerConfig;
    let (authored_root, context) = if consumer {
        (root.join(".mars-src"), PrepareContext::AuthoredLocal)
    } else {
        (root.clone(), PrepareContext::Package)
    };

    if !authored_root.is_dir() {
        if consumer && !json {
            output::print_info(&format!(
                "nothing to migrate: {} is consumer configuration, not a package; dependencies migrate in their own repositories",
                root.display()
            ));
        }
        return report(args.check, Vec::new(), Vec::new(), json);
    }

    let mut diagnostics = crate::diagnostic::DiagnosticCollector::new();
    let prepared =
        crate::package_prepare::prepare_authored(&authored_root, context, None, &mut diagnostics)?;
    diagnostics.fail_on_errors()?;
    let pending = pending_writes(root, &authored_root, &prepared);
    let dropped = dropped_keys(root, &authored_root, &prepared);

    if !args.check {
        for file in &pending {
            crate::platform::fs::atomic_write_file(&file.path, &file.bytes)?;
        }
    }
    report(args.check, pending, dropped, json)
}

/// The upgrade's diagnostics are what upgrading removed (legacy keys, skill tool rules);
/// each is listed with its file, none truncated.
fn dropped_keys(project_root: &Path, authored_root: &Path, prepared: &Prepared) -> Vec<DroppedKey> {
    let mut dropped = Vec::new();
    for (source_path, diagnostics) in &prepared.upgrade_diagnostics {
        let file = relative_path(project_root, &authored_root.join(source_path));
        for diagnostic in diagnostics {
            dropped.push(DroppedKey {
                file: file.clone(),
                code: diagnostic.code.clone(),
                message: diagnostic.message.clone(),
            });
        }
    }
    dropped.sort_by(|left, right| (&left.code, &left.file).cmp(&(&right.code, &right.file)));
    dropped
}

fn relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .expect("authored migration path is inside the project")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn pending_writes(
    project_root: &Path,
    authored_root: &Path,
    prepared: &Prepared,
) -> Vec<PendingWrite> {
    let mut pending = Vec::new();
    if let Some(manifest) = &prepared.manifest
        && let Some(source_path) = &manifest.source_path
    {
        push_change(
            project_root,
            authored_root,
            source_path,
            &manifest.normalized_bytes,
            &manifest.change,
            &mut pending,
        );
    }
    for definition in &prepared.definitions {
        push_change(
            project_root,
            authored_root,
            &definition.source_path,
            &definition.normalized_bytes,
            &definition.change,
            &mut pending,
        );
    }
    pending.sort_by(|left, right| left.relative.cmp(&right.relative));
    pending
}

fn push_change(
    project_root: &Path,
    authored_root: &Path,
    source_path: &str,
    bytes: &[u8],
    change: &ChangeRecord,
    pending: &mut Vec<PendingWrite>,
) {
    if *change == ChangeRecord::None {
        return;
    }
    let path = authored_root.join(source_path);
    let relative = relative_path(project_root, &path);
    pending.push(PendingWrite {
        path,
        relative,
        bytes: bytes.to_vec(),
        fully_rewritten: *change == ChangeRecord::FullyRewritten,
    });
}

fn report(
    check: bool,
    pending: Vec<PendingWrite>,
    dropped: Vec<DroppedKey>,
    json: bool,
) -> Result<i32, MarsError> {
    let files = pending
        .iter()
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    let fully_rewritten_files = pending
        .iter()
        .filter(|file| file.fully_rewritten)
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    if json {
        output::print_json(&MigrateReport {
            check,
            changed: !files.is_empty(),
            files,
            fully_rewritten_files,
            dropped,
        });
        return Ok(i32::from(check && !pending.is_empty()));
    }
    if files.is_empty() {
        output::print_info("definition format is already schema v1");
    } else {
        for file in &pending {
            let verb = match (check, file.fully_rewritten) {
                (true, true) => "would fully rewrite",
                (true, false) => "would edit",
                (false, true) => "fully rewrote",
                (false, false) => "edited",
            };
            output::print_info(&format!("{verb} {}", file.relative));
        }
    }
    let mut code = "";
    for key in &dropped {
        if key.code != code {
            code = &key.code;
            output::print_warn(&format!("{code}:"));
        }
        println!("    {}: {}", key.file, key.message);
    }
    Ok(i32::from(check && !pending.is_empty()))
}
