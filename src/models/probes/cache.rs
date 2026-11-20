//! The on-disk cache of one harness's model listing, shared by pi, opencode and cursor.
//!
//! One JSON file per harness under `availability/`, beside its own lock. A failed attempt never
//! replaces a usable last-good listing: it keeps `fetched_at` and `result` and only moves
//! `last_attempt_at` past `fetched_at`, which is how readers tell a stale-but-failed listing from
//! a stale-but-healthy one.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::probe_refresh::{ProbeCacheBranch, ProbeRefreshMode, resolve_probe_cache_branch};
use super::{Listing, ObservedOutcome, ProbeObservation};
use crate::error::MarsError;

const DEFAULT_TTL_SECS: u64 = 60;

/// A harness's probe result and where its cache lives.
pub trait CachedProbe: Clone + Serialize + DeserializeOwned {
    /// Name used in logs, the `__refresh-probe --target` argument and the installed set.
    const HARNESS: &'static str;
    const CACHE_FILE: &'static str;
    const LOCK_FILE: &'static str;
    /// Bump only when an older file must read as a miss.
    const SCHEMA_VERSION: u32;

    fn probe() -> Self;
    /// A listing that may stand as last-good evidence.
    fn is_usable(&self) -> bool;
    fn error(&self) -> Option<&str>;
    fn listing(&self) -> Listing;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry<R> {
    schema_version: u32,
    /// Written for every harness; absent in the files older builds wrote for opencode and cursor.
    /// Keep writing it: older builds read a `pi.json` without `harness: "pi"` as a miss.
    #[serde(default)]
    harness: String,
    fetched_at: u64,
    last_attempt_at: u64,
    last_error: Option<String>,
    result: Option<R>,
}

impl<R: CachedProbe> CacheEntry<R> {
    fn last_good(&self) -> Option<&R> {
        self.result.as_ref().filter(|result| result.is_usable())
    }

    fn latest_attempt_ok(&self) -> bool {
        self.last_attempt_at <= self.fetched_at
    }

    fn observation(&self) -> ProbeObservation {
        ProbeObservation {
            observed_at: Some(self.fetched_at),
            last_error: self.last_error.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CachedProbeOutcome<R> {
    Hit(R),
    Stale(R),
    Miss(R),
    /// Last good listing survived a newer failed attempt.
    StaleFailed(R),
    Failed(R),
    Unavailable,
}

impl<R> CachedProbeOutcome<R> {
    pub fn result(&self) -> Option<&R> {
        match self {
            Self::Hit(r)
            | Self::Stale(r)
            | Self::Miss(r)
            | Self::StaleFailed(r)
            | Self::Failed(r) => Some(r),
            Self::Unavailable => None,
        }
    }

    pub fn map<T>(self, f: impl FnOnce(R) -> T) -> CachedProbeOutcome<T> {
        match self {
            Self::Hit(r) => CachedProbeOutcome::Hit(f(r)),
            Self::Stale(r) => CachedProbeOutcome::Stale(f(r)),
            Self::Miss(r) => CachedProbeOutcome::Miss(f(r)),
            Self::StaleFailed(r) => CachedProbeOutcome::StaleFailed(f(r)),
            Self::Failed(r) => CachedProbeOutcome::Failed(f(r)),
            Self::Unavailable => CachedProbeOutcome::Unavailable,
        }
    }

    pub fn cache_status(&self) -> &'static str {
        match self {
            Self::Hit(_) => "hit",
            Self::Stale(_) | Self::StaleFailed(_) => "stale",
            Self::Miss(_) => "miss",
            Self::Failed(_) => "failed",
            Self::Unavailable => "skipped",
        }
    }

    pub fn latest_attempt_ok(&self) -> bool {
        matches!(self, Self::Hit(_) | Self::Stale(_) | Self::Miss(_))
    }
}

fn cache_path<R: CachedProbe>() -> Result<PathBuf, MarsError> {
    Ok(super::cache_dir()?.join(R::CACHE_FILE))
}

fn ttl_secs() -> u64 {
    std::env::var("MARS_PROBE_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TTL_SECS)
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_fresh<R>(entry: &CacheEntry<R>) -> bool {
    let now = now_unix_secs();
    entry.fetched_at <= now && (now - entry.fetched_at) < ttl_secs()
}

fn is_fresh_and_usable<R: CachedProbe>(entry: &CacheEntry<R>) -> bool {
    is_fresh(entry) && entry.last_good().is_some()
}

/// A file that is missing, unparseable or from another schema reads as no cache.
fn read_cache<R: CachedProbe>(path: &Path) -> Option<CacheEntry<R>> {
    let content = std::fs::read_to_string(path).ok()?;
    let entry: CacheEntry<R> = serde_json::from_str(&content).ok()?;
    (entry.schema_version == R::SCHEMA_VERSION).then_some(entry)
}

fn read_default_cache<R: CachedProbe>() -> Option<CacheEntry<R>> {
    read_cache(&cache_path::<R>().ok()?)
}

fn write_cache<R: CachedProbe>(path: &Path, entry: &CacheEntry<R>) {
    // Best effort: a probe that cannot be cached is run again next time.
    if let Ok(json) = serde_json::to_string_pretty(entry) {
        let _ = crate::fs::atomic_write(path, json.as_bytes());
    }
}

fn failure_message<R: CachedProbe>(result: &R) -> String {
    result
        .error()
        .map_or_else(|| format!("{} probe failed", R::HARNESS), str::to_string)
}

/// The one write decision, for foreground and detached refreshes alike. Returns the retained
/// last-good listing when a failed attempt had to keep it.
fn persist_probe_attempt<R: CachedProbe>(
    path: &Path,
    existing: Option<&CacheEntry<R>>,
    result: &R,
) -> Option<R> {
    let now = now_unix_secs();
    if !result.is_usable()
        && let Some(last_good) = existing.filter(|entry| entry.last_good().is_some())
    {
        write_cache(
            path,
            &CacheEntry {
                schema_version: R::SCHEMA_VERSION,
                harness: R::HARNESS.to_string(),
                fetched_at: last_good.fetched_at,
                last_attempt_at: now.max(last_good.fetched_at.saturating_add(1)),
                last_error: Some(failure_message(result)),
                result: last_good.result.clone(),
            },
        );
        return last_good.result.clone();
    }
    let usable = result.is_usable();
    write_cache(
        path,
        &CacheEntry {
            schema_version: R::SCHEMA_VERSION,
            harness: R::HARNESS.to_string(),
            fetched_at: if usable { now } else { 0 },
            last_attempt_at: now,
            last_error: (!usable).then(|| failure_message(result)),
            result: Some(result.clone()),
        },
    );
    None
}

/// The outcome of a probe this process just ran and has no last-good listing to fall back on.
fn fresh_probe_outcome<R: CachedProbe>(result: R) -> ObservedOutcome<CachedProbeOutcome<R>> {
    let usable = result.is_usable();
    let last_error = (!usable).then(|| failure_message(&result));
    ObservedOutcome {
        outcome: if usable {
            CachedProbeOutcome::Miss(result)
        } else {
            CachedProbeOutcome::Failed(result)
        },
        observation: Some(ProbeObservation {
            observed_at: Some(now_unix_secs()),
            last_error,
        }),
    }
}

/// The outcome for a cached last-good listing read without probing.
fn cached_outcome<R>(latest_attempt_ok: bool, result: R, fresh: bool) -> CachedProbeOutcome<R> {
    match (fresh, latest_attempt_ok) {
        (_, false) => CachedProbeOutcome::StaleFailed(result),
        (true, true) => CachedProbeOutcome::Hit(result),
        (false, true) => CachedProbeOutcome::Stale(result),
    }
}

pub fn probe_cached_listing<R: CachedProbe>(
    installed: &HashSet<String>,
    mars_offline: bool,
    probe_refresh: ProbeRefreshMode,
) -> ObservedOutcome<CachedProbeOutcome<Listing>> {
    let observed = probe_cached_observed::<R>(installed, mars_offline, probe_refresh);
    ObservedOutcome {
        outcome: observed.outcome.map(|result| result.listing()),
        observation: observed.observation,
    }
}

fn probe_cached_observed<R: CachedProbe>(
    installed: &HashSet<String>,
    mars_offline: bool,
    probe_refresh: ProbeRefreshMode,
) -> ObservedOutcome<CachedProbeOutcome<R>> {
    if mars_offline || !installed.contains(R::HARNESS) {
        return ObservedOutcome {
            outcome: CachedProbeOutcome::Unavailable,
            observation: None,
        };
    }

    let path = cache_path::<R>().ok();
    let cached = path.as_deref().and_then(read_cache::<R>);
    let observation = cached.as_ref().map(CacheEntry::observation);
    let latest_attempt_ok = cached.as_ref().is_none_or(CacheEntry::latest_attempt_ok);
    let branch = resolve_probe_cache_branch(
        cached,
        mars_offline,
        probe_refresh,
        |entry| entry.last_good(),
        is_fresh,
        trigger_background_refresh::<R>,
    );
    let outcome = match branch {
        ProbeCacheBranch::Hit(result) => cached_outcome(latest_attempt_ok, result, true),
        ProbeCacheBranch::Stale(result) => cached_outcome(latest_attempt_ok, result, false),
        ProbeCacheBranch::Unavailable => CachedProbeOutcome::Unavailable,
        ProbeCacheBranch::SynchronousProbe => {
            return synchronous_probe(path.as_deref());
        }
    };
    ObservedOutcome {
        outcome,
        observation,
    }
}

fn trigger_background_refresh<R: CachedProbe>() {
    let Some(lock) = super::probe_lock(R::LOCK_FILE) else {
        return;
    };
    if !read_default_cache::<R>().is_some_and(|entry| is_fresh_and_usable(&entry)) {
        let _ = spawn_refresh_probe(R::HARNESS);
    }
    drop(lock);
}

fn spawn_refresh_probe(target: &str) -> Result<(), ()> {
    let program = std::env::current_exe().map_err(|_| ())?;
    crate::platform::process::spawn_detached(
        program.as_os_str(),
        &[
            OsString::from("models"),
            OsString::from("__refresh-probe"),
            OsString::from("--target"),
            OsString::from(target),
        ],
    )
    .map_err(|_| ())
}

fn synchronous_probe<R: CachedProbe>(
    path: Option<&Path>,
) -> ObservedOutcome<CachedProbeOutcome<R>> {
    let lock = super::probe_lock_blocking(R::LOCK_FILE);
    let existing = path.and_then(read_cache::<R>);

    let Some(_lock) = lock else {
        // Without the lock, do not overwrite a last-good listing or race a background writer.
        // It is still useful as support evidence.
        if let Some(entry) = &existing
            && let Some(result) = entry.last_good()
        {
            return ObservedOutcome {
                outcome: cached_outcome(entry.latest_attempt_ok(), result.clone(), false),
                observation: Some(entry.observation()),
            };
        }
        return fresh_probe_outcome(R::probe());
    };

    if let Some(entry) = &existing
        && let Some(result) = entry.last_good()
    {
        if is_fresh(entry) {
            return ObservedOutcome {
                outcome: cached_outcome(entry.latest_attempt_ok(), result.clone(), true),
                observation: Some(entry.observation()),
            };
        }
        let probed = R::probe();
        let retained =
            path.and_then(|path| persist_probe_attempt(path, existing.as_ref(), &probed));
        return match retained {
            Some(last_good) => ObservedOutcome {
                outcome: CachedProbeOutcome::StaleFailed(last_good),
                observation: Some(ProbeObservation {
                    observed_at: Some(entry.fetched_at),
                    last_error: Some(failure_message(&probed)),
                }),
            },
            None => fresh_probe_outcome(probed),
        };
    }

    let probed = R::probe();
    if let Some(path) = path {
        persist_probe_attempt(path, None, &probed);
    }
    fresh_probe_outcome(probed)
}

/// `meridian models __refresh-probe --target <harness>`: the detached worker.
pub fn run_refresh_probe_command<R: CachedProbe>() -> Result<i32, MarsError> {
    let Some(_lock) = super::probe_lock_blocking(R::LOCK_FILE) else {
        return Ok(0);
    };

    let existing = read_default_cache::<R>();
    if existing.as_ref().is_some_and(is_fresh_and_usable) {
        return Ok(0);
    }

    let probed = R::probe();
    if let Ok(path) = cache_path::<R>() {
        persist_probe_attempt(&path, existing.as_ref(), &probed);
    }
    Ok(0)
}
