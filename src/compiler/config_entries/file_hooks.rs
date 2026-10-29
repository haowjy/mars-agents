//! File-placement hook outputs (OpenCode, Pi): which files the lock says a hook owns, and
//! writing them.

use std::collections::HashMap;

use crate::diagnostic::DiagnosticCollector;
use crate::sync::package_dir::PackageDir;
use crate::types::MarsContext;
use mars_schema::item::ItemKind;

/// The files the old lock says hooks own on file-placement targets: every output of a hook
/// item on such a target except its directory copy, which target sync owns. A hook may own
/// several files on one target; none is derived from its name.
pub(crate) fn file_hook_output_preserve_paths(
    lock: &crate::lock::LockFile,
) -> HashMap<String, std::collections::HashSet<String>> {
    let mut preserve = HashMap::<String, std::collections::HashSet<String>>::new();
    for item in lock
        .items
        .values()
        .filter(|item| item.kind == ItemKind::Hook)
    {
        for (target_root, path) in owned_files(item) {
            preserve.entry(target_root).or_default().insert(path);
        }
    }
    preserve
}

/// The files, relative to `dir`'s target root, that the hook which installed `dir` owns
/// there. A kept hook's outputs stay as they were.
pub(super) fn kept_file_outputs(lock: &crate::lock::LockFile, dir: &PackageDir) -> Vec<String> {
    let canonical = dir.canonical();
    lock.items
        .values()
        .filter(|item| {
            item.outputs.iter().any(|output| {
                output.target_root == crate::lock::CANONICAL_TARGET_ROOT
                    && output.dest_path == canonical
            })
        })
        .flat_map(owned_files)
        .filter(|(target_root, _)| dir.belongs_to(target_root))
        .map(|(_, path)| path)
        .collect()
}

/// `(target root, path)` of each file `item` owns on a file-placement target: its non-
/// canonical outputs there, minus the directory copy.
fn owned_files(item: &crate::lock::LockedItemV2) -> Vec<(String, String)> {
    // A hook with no canonical output is a retry tombstone; target sync retries it.
    let dir_copies: Vec<PackageDir> = item
        .outputs
        .iter()
        .filter(|output| output.target_root == crate::lock::CANONICAL_TARGET_ROOT)
        .filter_map(|output| PackageDir::parse_canonical(ItemKind::Hook, &output.dest_path))
        .collect();
    if dir_copies.is_empty() {
        return Vec::new();
    }
    item.outputs
        .iter()
        .filter(|output| output.target_root != crate::lock::CANONICAL_TARGET_ROOT)
        .filter(|output| {
            mars_schema::hooks::targets::target(&output.target_root)
                .is_some_and(mars_schema::hooks::targets::Target::is_file)
        })
        .filter(|output| {
            !dir_copies.iter().any(|dir| {
                dir.belongs_to(&output.target_root)
                    && crate::target::dest_paths_equivalent(
                        &dir.native(),
                        output.dest_path.as_str(),
                    )
            })
        })
        .map(|output| {
            (
                output.target_root.clone(),
                output.dest_path.as_str().to_string(),
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_file_hook_outputs(
    write: crate::surface_ownership::retention::FileHookWrite<'_>,
    ctx: &MarsContext,
    old_lock: &crate::lock::LockFile,
    ownership_lock: &mut crate::lock::LockFile,
    force: bool,
    diag: &mut DiagnosticCollector,
    emitted_outputs: &mut Vec<crate::lock::CompiledNativeOutput>,
) {
    let target_root = write.target_root().to_owned();
    let (target_dir, files) = write.into_parts(&ctx.project_root);
    for (owner_canonical_dest_path, relative, content) in files {
        let path = target_dir.join(&relative);
        let dest_exists = crate::surface_ownership::target_dest_exists(&path);
        match crate::surface_ownership::copy_decision(
            old_lock,
            &target_root,
            &relative,
            dest_exists,
            force,
        ) {
            crate::surface_ownership::SurfaceCopyDecision::SkipWithoutInstalledClaim => {
                crate::surface_ownership::warn_no_installed_claim_collision(
                    &target_root,
                    &relative,
                    crate::surface_ownership::CollisionAdoptHint::SyncForce,
                    diag,
                );
                continue;
            }
            crate::surface_ownership::SurfaceCopyDecision::Proceed => {
                if dest_exists
                    && force
                    && old_lock
                        .installed_checksum_for_output(&target_root, &relative)
                        .is_none()
                {
                    crate::surface_ownership::warn_no_installed_claim_adopted(
                        &target_root,
                        &relative,
                        crate::surface_ownership::CollisionAdoptHint::SyncForce,
                        diag,
                    );
                }
            }
        }
        if dest_exists && !force {
            let previous = old_lock.items.values().find_map(|item| {
                item.outputs.iter().find(|output| {
                    output.target_root == target_root
                        && crate::target::dest_paths_equivalent(
                            output.dest_path.as_str(),
                            &relative,
                        )
                })
            });
            let diverged = previous.is_some_and(|previous| {
                std::fs::read(&path)
                    .map(|bytes| crate::hash::hash_bytes(&bytes))
                    .is_ok_and(|actual| {
                        previous
                            .installed_checksum()
                            .is_none_or(|checksum| actual != checksum.as_ref())
                    })
            });
            if diverged {
                diag.warn(
                    "target-divergent",
                    format!(
                        "target `{target_root}` item `{relative}` was edited after Mars installed it \
                         (preserved local content; run `{}` to reset)",
                        crate::types::managed_cmd("mars sync --force")
                    ),
                );
                continue;
            }
        }
        let output = crate::lock::CompiledNativeOutput {
            owner_canonical_dest_path,
            target_root: target_root.clone(),
            dest_path: relative,
            installed_checksum: crate::types::ContentHash::from(crate::hash::hash_bytes(
                content.as_bytes(),
            )),
        };
        if let Err(error) = crate::lock::apply_compiled_native_outputs(
            ownership_lock,
            std::slice::from_ref(&output),
        ) {
            diag.warn(
                "config-entry-write",
                format!(
                    "not writing file hook `{}` because its ownership cannot be recorded: {error}",
                    path.display()
                ),
            );
            continue;
        }
        let unchanged =
            dest_exists && std::fs::read(&path).is_ok_and(|bytes| bytes == content.as_bytes());
        let result = (|| -> Result<(), crate::error::MarsError> {
            if unchanged {
                return Ok(());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::fs::atomic_write(&path, content.as_bytes())
        })();
        if let Err(error) = result {
            diag.warn(
                "config-entry-write",
                format!("failed to write file hook `{}`: {error}", path.display()),
            );
            continue;
        }
        emitted_outputs.push(output);
    }
}
