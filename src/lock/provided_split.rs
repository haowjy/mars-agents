//! Which records of a lock belong in `provided-outputs.lock` rather than `mars.lock`.
//!
//! `mars.lock` is committed and shared; the records of provided content name a path of one
//! machine and exist only where the host is installed, so they live in a machine-local file of
//! the same schema. [`split`] and [`merge`] are inverses over the lock a sync sees.

use super::LockFile;
use crate::provided::is_provided_source;

/// `(committed, machine-local)`.
pub(super) fn split(lock: &LockFile) -> (LockFile, LockFile) {
    let mut committed = lock.clone();
    let mut local = LockFile::empty();
    local.version = lock.version;
    committed.items.retain(|key, item| {
        let provided = is_provided_source(item.source.as_ref());
        if provided {
            local.items.insert(key.clone(), item.clone());
        }
        !provided
    });
    committed.dependencies.retain(|name, dependency| {
        let provided = is_provided_source(name.as_ref());
        if provided {
            local.dependencies.insert(name.clone(), dependency.clone());
        }
        !provided
    });
    (committed, local)
}

/// A key both files hold takes the machine-local value.
pub(super) fn merge(mut committed: LockFile, local: LockFile) -> LockFile {
    committed.items.extend(local.items);
    committed.dependencies.extend(local.dependencies);
    for (target, records) in local.config_entries {
        committed
            .config_entries
            .entry(target)
            .or_default()
            .extend(records);
    }
    committed
}

impl LockFile {
    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.dependencies.is_empty() && self.config_entries.is_empty()
    }
}
