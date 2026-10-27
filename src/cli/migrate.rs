//! `mars migrate` — apply crate-owned authored-file changes for schema v1.

use std::path::{Path, PathBuf};

use mars_schema::prepare::{ChangeRecord, PrepareContext, Prepared, project_authored_root};
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
    dropped: Vec<Notice>,
    /// What the migration left alone and why, by file.
    warnings: Vec<Notice>,
}

#[derive(Serialize)]
struct Notice {
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
    let (authored_root, context) = project_authored_root(root, Some(&manifest));
    let consumer = context == PrepareContext::AuthoredLocal;

    if !authored_root.is_dir() {
        if consumer && !json {
            output::print_info(&format!(
                "nothing to migrate: {} is consumer configuration, not a package; dependencies migrate in their own repositories",
                root.display()
            ));
        }
        return report(args.check, Vec::new(), Vec::new(), Vec::new(), json);
    }

    let mut diagnostics = crate::diagnostic::DiagnosticCollector::new();
    let prepared = crate::package_prepare::prepare_authored_for_migration(
        &authored_root,
        context,
        &mut diagnostics,
    )?;
    diagnostics.fail_on_errors()?;
    let (pending, warnings) = pending_writes(root, &authored_root, &prepared, !consumer)?;
    let dropped = dropped_keys(root, &authored_root, &prepared);

    if !args.check {
        for file in &pending {
            crate::platform::fs::atomic_write_file(&file.path, &file.bytes)?;
        }
    }
    report(args.check, pending, dropped, warnings, json)
}

/// The upgrade's diagnostics are what upgrading removed (legacy keys, skill tool rules);
/// each is listed with its file, none truncated.
fn dropped_keys(project_root: &Path, authored_root: &Path, prepared: &Prepared) -> Vec<Notice> {
    let mut dropped = Vec::new();
    for (source_path, diagnostics) in &prepared.changes {
        let file = relative_path(project_root, &authored_root.join(source_path));
        for diagnostic in diagnostics {
            dropped.push(Notice {
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

/// The manifest travels through `prepare`'s normalized bytes, which carry the pre-v1 upgrade, and
/// then through the one key `migrate` adds: a `requires-mars` floor when the package has none.
/// Every other write is a definition the upgrade or a translation changed.
fn pending_writes(
    project_root: &Path,
    authored_root: &Path,
    prepared: &Prepared,
    package: bool,
) -> Result<(Vec<PendingWrite>, Vec<Notice>), MarsError> {
    let mut pending = Vec::new();
    let mut warnings = Vec::new();
    if package
        && let Some(manifest) = prepared.manifest.as_ref()
        && let Some(source_path) = manifest.source_path.as_deref()
    {
        let path = authored_root.join(source_path);
        let relative = relative_path(project_root, &path);
        let normalized = std::str::from_utf8(&manifest.normalized_bytes)
            .expect("preparation refuses a manifest that is not UTF-8");
        match mars_schema::upgrade::ensure_requires_mars_floor(normalized) {
            Ok(outcome) => {
                if let Some(message) = outcome.warning {
                    warnings.push(Notice {
                        file: relative.clone(),
                        code: "requires-mars-floor".to_string(),
                        message,
                    });
                }
                if outcome.text != std::fs::read_to_string(&path)? {
                    pending.push(PendingWrite {
                        path,
                        relative,
                        bytes: outcome.text.into_bytes(),
                        fully_rewritten: false,
                    });
                }
            }
            // A package an earlier Mars migrated succeeded here; a manifest shape this step
            // cannot inspect is left alone like an inline table, never turned into a failure.
            Err(error) => warnings.push(Notice {
                file: relative,
                code: "requires-mars-floor".to_string(),
                message: error.to_string(),
            }),
        }
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
    Ok((pending, warnings))
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
    dropped: Vec<Notice>,
    warnings: Vec<Notice>,
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
            warnings,
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
    for warning in &warnings {
        output::print_warn(&format!("{}: {}", warning.file, warning.message));
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
