use std::collections::HashSet;
use std::path::PathBuf;

use crate::error::MarsError;

pub mod cursor;
pub mod cursor_cache;
pub mod opencode;
pub mod opencode_cache;
pub mod pi;
pub mod pi_cache;
pub mod probe_refresh;

pub use probe_refresh::ProbeRefreshMode;

pub use cursor::CursorProbeResult;
pub use opencode::OpenCodeProbeResult;
pub use pi::PiProbeResult;

/// The shared probe-cache directory: one file per host's last-known-good listing.
pub(super) fn cache_dir() -> Result<PathBuf, MarsError> {
    Ok(crate::platform::cache::global_cache_root()?.join("availability"))
}

/// A probe cache's lock, best effort: `None` when another process holds it or the path cannot be
/// opened, so the caller skips the refresh instead of failing. An open error is logged — it is not
/// contention, and a silent skip would hide a broken cache directory.
pub(super) fn probe_lock(file: &str, label: &str) -> Option<crate::fs::FileLock> {
    let dir = cache_dir().ok()?;
    match crate::fs::FileLock::try_acquire(&dir.join(file)) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("debug: {label} probe cache lock failed: {error}");
            None
        }
    }
}

/// Wait for a probe cache's lock. `None` when the path cannot be opened: the cache is best effort.
pub(super) fn probe_lock_blocking(file: &str) -> Option<crate::fs::FileLock> {
    let dir = cache_dir().ok()?;
    crate::fs::FileLock::acquire(&dir.join(file)).ok()
}

/// Provenance captured with the outcome from the same cache read or probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeObservation {
    pub observed_at: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ObservedOutcome<T> {
    pub outcome: T,
    pub observation: Option<ProbeObservation>,
}

/// Determine whether an OpenCode probe should be attempted.
/// Returns false if offline or opencode is not installed.
pub fn should_probe_opencode(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("opencode")
}

/// Determine whether a cursor probe should be attempted.
/// Returns false if offline or cursor is not installed.
pub fn should_probe_cursor(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("cursor")
}
