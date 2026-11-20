use std::collections::HashSet;
use std::path::PathBuf;

use crate::error::MarsError;
use cache::{CachedProbe, CachedProbeOutcome};

pub mod agy;
pub mod cache;
mod command;
pub mod cursor;
pub mod opencode;
pub mod pi;
pub mod probe_refresh;

pub use probe_refresh::ProbeRefreshMode;

pub use agy::AgyProbeResult;
pub use cursor::CursorProbeResult;
pub use opencode::OpenCodeProbeResult;
pub use pi::PiProbeResult;

/// What every probe-backed harness's result says, whatever it also records. Callers outside
/// the probe modules read this, never a harness's own result type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub slugs: Vec<String>,
    /// The harness's model listing command completed.
    pub succeeded: bool,
    pub error: Option<String>,
    /// Why the harness cannot be driven at all (a skip reason code), even when its listing
    /// completed. Pi sets this when its help surface lacks a flag Mars needs.
    pub incompatible: Option<&'static str>,
    /// A completed but empty listing is no evidence of support or credentials (Cursor).
    pub empty_is_failure: bool,
}

impl Listing {
    /// Whether the listing counts as evidence for routing: it completed and, where an empty
    /// one proves nothing, listed something.
    pub fn is_evidence(&self) -> bool {
        self.succeeded && !(self.empty_is_failure && self.slugs.is_empty())
    }
}

/// One probe-backed harness's cache and refresh, with the result type erased to a [`Listing`].
#[derive(Debug, Clone, Copy)]
pub struct ProbeDriver {
    load: fn(
        &HashSet<String>,
        bool,
        ProbeRefreshMode,
    ) -> ObservedOutcome<CachedProbeOutcome<Listing>>,
    refresh: fn() -> Result<i32, MarsError>,
}

impl ProbeDriver {
    pub const fn of<R: CachedProbe>() -> Self {
        Self {
            load: cache::probe_cached_listing::<R>,
            refresh: cache::run_refresh_probe_command::<R>,
        }
    }

    /// The cached listing, probing when the cache and `probe_refresh` call for it.
    pub fn load(
        &self,
        installed: &HashSet<String>,
        mars_offline: bool,
        probe_refresh: ProbeRefreshMode,
    ) -> ObservedOutcome<CachedProbeOutcome<Listing>> {
        (self.load)(installed, mars_offline, probe_refresh)
    }

    /// The detached `__refresh-probe` worker.
    pub fn refresh(&self) -> Result<i32, MarsError> {
        (self.refresh)()
    }
}

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
