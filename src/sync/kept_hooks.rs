//! A dependency hook whose `hook.toml` fails validation keeps what the last sync installed
//! (D19): the invalid hook is found in the old lock once the target state is complete, and
//! `hook-invalid-kept` says what happened to it.

use std::path::Path;

use indexmap::IndexMap;
use mars_schema::item::ItemKind;

use crate::compiler::hooks::HookRank;
use crate::diagnostic::DiagnosticCollector;
use crate::lock::{CANONICAL_TARGET_ROOT, LockFile, LockIndex};
use crate::sync::package_dir::PackageDir;
use crate::sync::target::TargetItem;
use crate::types::{DestPath, SourceName};

/// A dependency hook whose `hook.toml` failed validation and the consumer selected.
#[derive(Debug, Clone)]
pub(crate) struct InvalidDependencyHook {
    pub source_name: SourceName,
    /// The hook's directory under its package root, as `mars.lock` records it (`lock_source_path`).
    pub source_path: Option<String>,
    /// Names the last sync may have installed the hook under, most likely first.
    pub installed_names: Vec<String>,
    /// Where the hook sorted in the last sync, so its kept entries keep their place.
    pub rank: HookRank,
    pub message: String,
}

/// A dependency hook whose `hook.toml` no longer validates. It keeps what the previous lock
/// installed: its per-target directories (never copied from the invalid source), and,
/// downstream, its merge entries and file outputs. See `compiler::config_entries`.
#[derive(Debug, Clone)]
pub(crate) struct KeptHook {
    pub name: String,
    pub rank: HookRank,
    pub dirs: Vec<PackageDir>,
}

/// A hook directory's path under its package root, the way `mars.lock` records it: `hooks/guard`,
/// separated by `/` on every platform.
pub(super) fn lock_source_path(package_root: &Path, hook_dir: &Path) -> Option<String> {
    let relative = hook_dir.strip_prefix(package_root).ok()?;
    Some(
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// The names the old lock installed the hook at `source_path` of `source` under. The path
/// survives a `hook.toml` that no longer says its name; a lock written before the path was
/// recorded has none, and the file's own names are the fallback.
fn names_installed_from(
    old_lock: &LockFile,
    source: &SourceName,
    source_path: &str,
) -> Vec<String> {
    let mut names = Vec::new();
    for item in old_lock.items.values().filter(|item| {
        item.kind == ItemKind::Hook
            && &item.source == source
            && item.source_path.as_deref() == Some(source_path)
    }) {
        for output in item
            .outputs
            .iter()
            .filter(|output| output.target_root == CANONICAL_TARGET_ROOT)
        {
            if let Some(dir) = PackageDir::parse_canonical(ItemKind::Hook, &output.dest_path)
                && !names.iter().any(|name| name == dir.name())
            {
                names.push(dir.name().to_owned());
            }
        }
    }
    names
}

/// Decide what each invalid dependency hook keeps from `old_lock` and say so; `None` (a
/// build with no previous install) keeps nothing. `items` is the complete target state: a
/// directory it already holds is not the kept hook's.
pub(super) fn settle(
    invalid_hooks: Vec<InvalidDependencyHook>,
    items: &IndexMap<DestPath, TargetItem>,
    old_lock: Option<&LockFile>,
    managed_targets: &[String],
    diag: &mut DiagnosticCollector,
) -> Vec<KeptHook> {
    let lock_index = old_lock.map(LockIndex::new);
    let mut kept_hooks = Vec::new();
    for invalid in invalid_hooks {
        // The hook is whatever the old lock installed from this package: at its directory,
        // else under a name its file may still declare.
        let mut candidates = old_lock
            .zip(invalid.source_path.as_deref())
            .map(|(lock, path)| names_installed_from(lock, &invalid.source_name, path))
            .unwrap_or_default();
        candidates.extend(invalid.installed_names.iter().cloned());
        let kept = candidates.iter().find_map(|name| {
            let dirs: Vec<PackageDir> = managed_targets
                .iter()
                .filter_map(|target| PackageDir::new(ItemKind::Hook, target, name))
                .filter(|dir| {
                    let dest = dir.canonical();
                    !items.contains_key(&dest)
                        && lock_index
                            .as_ref()
                            .and_then(|index| index.find_output(CANONICAL_TARGET_ROOT, &dest))
                            .is_some_and(|locked| locked.source == invalid.source_name)
                })
                .collect();
            (!dirs.is_empty()).then(|| KeptHook {
                name: name.clone(),
                rank: invalid.rank,
                dirs,
            })
        });
        let outcome = if kept.is_some() {
            "keeping what the last sync installed"
        } else {
            "no installed copy of it was found, so nothing is installed"
        };
        diag.warn(
            "hook-invalid-kept",
            format!("{}; {outcome}", invalid.message),
        );
        kept_hooks.extend(kept);
    }
    kept_hooks
}
