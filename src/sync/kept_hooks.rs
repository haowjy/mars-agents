//! A dependency hook whose `hook.toml` fails validation keeps what the last sync installed
//! (D19): the invalid hook is found in the old lock once the target state is complete, and
//! `hook-invalid-kept` says what happened to it.

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
        // The hook is whatever the old lock installed from this package under one of its
        // names: its file no longer says which.
        let kept = invalid.installed_names.iter().find_map(|name| {
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
