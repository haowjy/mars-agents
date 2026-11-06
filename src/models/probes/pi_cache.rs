use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::pi::PiProbeResult;
use super::probe_refresh::ProbeCacheBranch;
use crate::error::MarsError;

const SCHEMA_VERSION: u32 = 3;
const DEFAULT_TTL_SECS: u64 = 60;
const LOCK_FILE: &str = ".pi.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiProbeCacheEntry {
    pub schema_version: u32,
    pub harness: String,
    pub fetched_at: u64,
    pub last_attempt_at: u64,
    pub last_error: Option<String>,
    pub result: Option<PiProbeResult>,
}

#[derive(Debug, Clone)]
pub enum CachedPiProbeOutcome {
    Hit(PiProbeResult),
    Stale(PiProbeResult),
    Miss(PiProbeResult),
    /// Last good listing survived a newer failed attempt.
    StaleFailed(PiProbeResult),
    Failed(PiProbeResult),
    Unavailable,
}

impl CachedPiProbeOutcome {
    pub fn result(&self) -> Option<&PiProbeResult> {
        match self {
            Self::Hit(r)
            | Self::Stale(r)
            | Self::Miss(r)
            | Self::StaleFailed(r)
            | Self::Failed(r) => Some(r),
            Self::Unavailable => None,
        }
    }

    pub fn cache_status(&self) -> &'static str {
        match self {
            Self::Hit(_) => "hit",
            Self::Stale(_) => "stale",
            Self::Miss(_) => "miss",
            Self::StaleFailed(_) => "stale",
            Self::Failed(_) => "failed",
            Self::Unavailable => "skipped",
        }
    }

    pub fn latest_attempt_ok(&self) -> bool {
        matches!(self, Self::Hit(_) | Self::Stale(_) | Self::Miss(_))
    }
}

fn should_probe_pi(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("pi")
}

fn cache_path() -> Result<PathBuf, MarsError> {
    Ok(super::cache_dir()?.join("pi.json"))
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

fn is_fresh(entry: &PiProbeCacheEntry) -> bool {
    let ttl = ttl_secs();
    let now = now_unix_secs();
    if entry.fetched_at > now {
        return false;
    }
    (now - entry.fetched_at) < ttl
}

fn read_cache_tolerant() -> Option<PiProbeCacheEntry> {
    read_cache_tolerant_at(&cache_path().ok()?)
}

fn read_cache_tolerant_at(path: &Path) -> Option<PiProbeCacheEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let entry: PiProbeCacheEntry = serde_json::from_str(&content).ok()?;
    if entry.schema_version != SCHEMA_VERSION {
        return None;
    }
    if !entry.harness.eq_ignore_ascii_case("pi") {
        return None;
    }
    Some(entry)
}

fn write_cache_at(path: &Path, entry: &PiProbeCacheEntry) -> Result<(), MarsError> {
    let json = serde_json::to_string_pretty(entry)
        .map_err(|e| MarsError::Internal(format!("pi probe cache serialize: {e}")))?;
    crate::fs::atomic_write(path, json.as_bytes())
}

pub fn probe_cached(
    installed: &HashSet<String>,
    mars_offline: bool,
    probe_refresh: super::ProbeRefreshMode,
) -> CachedPiProbeOutcome {
    probe_cached_observed(installed, mars_offline, probe_refresh).outcome
}

pub fn probe_cached_observed(
    installed: &HashSet<String>,
    mars_offline: bool,
    probe_refresh: super::ProbeRefreshMode,
) -> super::ObservedOutcome<CachedPiProbeOutcome> {
    if !should_probe_pi(installed, mars_offline) {
        return super::ObservedOutcome {
            outcome: CachedPiProbeOutcome::Unavailable,
            observation: None,
        };
    }

    probe_cached_impl_observed(
        mars_offline,
        probe_refresh,
        &cache_path().ok(),
        super::pi::probe,
        || {
            let program = std::env::current_exe().map_err(|_| ())?;
            crate::platform::process::spawn_detached(
                program.as_os_str(),
                &[
                    OsString::from("models"),
                    OsString::from("__refresh-probe"),
                    OsString::from("--target"),
                    OsString::from("pi"),
                ],
            )
            .map_err(|_| ())
        },
    )
}

fn probe_cached_impl_observed<F, S>(
    mars_offline: bool,
    probe_refresh: super::ProbeRefreshMode,
    path: &Option<PathBuf>,
    probe: F,
    spawn_refresh: S,
) -> super::ObservedOutcome<CachedPiProbeOutcome>
where
    F: Fn() -> PiProbeResult,
    S: Fn() -> Result<(), ()>,
{
    let cached = path.as_deref().and_then(read_cache_tolerant_at);
    let observation = cached.as_ref().map(observation_from_entry);
    let latest_attempt_ok = cached
        .as_ref()
        .is_none_or(|entry| entry.last_attempt_at <= entry.fetched_at);
    let outcome = match super::probe_refresh::resolve_probe_cache_branch(
        cached,
        mars_offline,
        probe_refresh,
        |entry| {
            entry
                .result
                .as_ref()
                .filter(|result| is_usable_result(Some(*result)))
        },
        is_fresh,
        || trigger_background_refresh_with(spawn_refresh),
    ) {
        ProbeCacheBranch::Hit(result) => {
            if latest_attempt_ok {
                CachedPiProbeOutcome::Hit(result)
            } else {
                CachedPiProbeOutcome::StaleFailed(result)
            }
        }
        ProbeCacheBranch::Stale(result) => {
            if latest_attempt_ok {
                CachedPiProbeOutcome::Stale(result)
            } else {
                CachedPiProbeOutcome::StaleFailed(result)
            }
        }
        ProbeCacheBranch::Unavailable => CachedPiProbeOutcome::Unavailable,
        ProbeCacheBranch::SynchronousProbe => {
            return synchronous_probe_with_observation(path, probe);
        }
    };
    super::ObservedOutcome {
        outcome,
        observation,
    }
}

fn observation_from_entry(entry: &PiProbeCacheEntry) -> super::ProbeObservation {
    super::ProbeObservation {
        observed_at: Some(entry.fetched_at),
        last_error: entry.last_error.clone(),
    }
}

fn trigger_background_refresh_with<S>(spawn_refresh: S)
where
    S: Fn() -> Result<(), ()>,
{
    let Some(lock) = super::probe_lock(LOCK_FILE, "pi") else {
        return;
    };
    if let Some(entry) = read_cache_tolerant()
        && is_fresh(&entry)
        && is_usable_result(entry.result.as_ref())
    {
        drop(lock);
        return;
    }
    let _ = spawn_refresh();
    drop(lock);
}

fn synchronous_probe_with_observation<F>(
    path: &Option<PathBuf>,
    probe: F,
) -> super::ObservedOutcome<CachedPiProbeOutcome>
where
    F: Fn() -> PiProbeResult,
{
    use super::ObservedOutcome;
    let lock = super::probe_lock_blocking(LOCK_FILE);
    if lock.is_none() {
        // Without the lock, do not overwrite a last-good listing or race a
        // background writer. It is still useful as support evidence.
        if let Some(entry) = path.as_deref().and_then(read_cache_tolerant_at)
            && is_usable_result(entry.result.as_ref())
        {
            let observation = Some(observation_from_entry(&entry));
            let outcome = if entry.last_attempt_at <= entry.fetched_at {
                CachedPiProbeOutcome::Stale(entry.result.unwrap())
            } else {
                CachedPiProbeOutcome::StaleFailed(entry.result.unwrap())
            };
            return ObservedOutcome {
                outcome,
                observation,
            };
        }
        let probe_result = probe();
        let last_error = if probe_result.model_probe_success && probe_result.error.is_none() {
            None
        } else {
            Some(
                probe_result
                    .error
                    .clone()
                    .unwrap_or_else(|| "pi probe failed".to_string()),
            )
        };
        let outcome = if probe_result.model_probe_success && probe_result.error.is_none() {
            CachedPiProbeOutcome::Miss(probe_result)
        } else {
            CachedPiProbeOutcome::Failed(probe_result)
        };
        return ObservedOutcome {
            outcome,
            observation: Some(super::ProbeObservation {
                observed_at: Some(now_unix_secs()),
                last_error,
            }),
        };
    }

    if lock.is_some()
        && let Some(path) = path
        && let Some(entry) = read_cache_tolerant_at(path)
        && is_usable_result(entry.result.as_ref())
    {
        if is_fresh(&entry) {
            let observation = Some(observation_from_entry(&entry));
            let outcome = if entry.last_attempt_at <= entry.fetched_at {
                CachedPiProbeOutcome::Hit(entry.result.unwrap())
            } else {
                CachedPiProbeOutcome::StaleFailed(entry.result.unwrap())
            };
            return ObservedOutcome {
                outcome,
                observation,
            };
        }

        let probe_result = probe();
        let retained = persist_probe_attempt(path, Some(&entry), &probe_result);
        let observation = if retained.is_some() {
            Some(super::ProbeObservation {
                observed_at: Some(entry.fetched_at),
                last_error: Some(
                    probe_result
                        .error
                        .clone()
                        .unwrap_or_else(|| "pi probe failed".to_string()),
                ),
            })
        } else {
            Some(super::ProbeObservation {
                observed_at: Some(now_unix_secs()),
                last_error: None,
            })
        };
        let outcome = match retained {
            Some(last_good) => CachedPiProbeOutcome::StaleFailed(last_good),
            None => CachedPiProbeOutcome::Miss(probe_result),
        };
        return ObservedOutcome {
            outcome,
            observation,
        };
    }

    let probe_result = probe();
    if let Some(path) = path {
        persist_probe_attempt(path, None, &probe_result);
    }
    drop(lock);

    let last_error = if probe_result.model_probe_success && probe_result.error.is_none() {
        None
    } else {
        Some(
            probe_result
                .error
                .clone()
                .unwrap_or_else(|| "pi probe failed".to_string()),
        )
    };
    let outcome = if probe_result.model_probe_success && probe_result.error.is_none() {
        CachedPiProbeOutcome::Miss(probe_result)
    } else {
        CachedPiProbeOutcome::Failed(probe_result)
    };
    ObservedOutcome {
        outcome,
        observation: Some(super::ProbeObservation {
            observed_at: Some(now_unix_secs()),
            last_error,
        }),
    }
}

fn write_failed_attempt(path: &Path, existing: &PiProbeCacheEntry, failed: &PiProbeResult) {
    let entry = PiProbeCacheEntry {
        schema_version: SCHEMA_VERSION,
        harness: "pi".to_string(),
        fetched_at: existing.fetched_at,
        last_attempt_at: now_unix_secs().max(existing.fetched_at.saturating_add(1)),
        last_error: Some(
            failed
                .error
                .clone()
                .unwrap_or_else(|| "pi probe failed".to_string()),
        ),
        result: existing.result.clone(),
    };
    if let Err(e) = write_cache_at(path, &entry) {
        eprintln!("debug: pi probe cache write failed: {e}");
    }
}

fn write_probe_attempt(path: &Path, probe_result: PiProbeResult) {
    let now = now_unix_secs();
    let succeeded = probe_result.model_probe_success && probe_result.error.is_none();
    let entry = PiProbeCacheEntry {
        schema_version: SCHEMA_VERSION,
        harness: "pi".to_string(),
        fetched_at: if succeeded { now } else { 0 },
        last_attempt_at: now,
        last_error: if succeeded {
            None
        } else {
            Some(
                probe_result
                    .error
                    .clone()
                    .unwrap_or_else(|| "pi probe failed".to_string()),
            )
        },
        result: Some(probe_result),
    };

    if let Err(e) = write_cache_at(path, &entry) {
        eprintln!("debug: pi probe cache write failed: {e}");
    }
}

/// The same write decision is used by foreground and detached refreshes.
/// A failed attempt cannot replace a usable last-good model listing.
fn persist_probe_attempt(
    path: &Path,
    existing: Option<&PiProbeCacheEntry>,
    result: &PiProbeResult,
) -> Option<PiProbeResult> {
    if !is_usable_result(Some(result))
        && let Some(last_good) = existing.filter(|entry| is_usable_result(entry.result.as_ref()))
    {
        write_failed_attempt(path, last_good, result);
        return last_good.result.clone();
    }
    write_probe_attempt(path, result.clone());
    None
}

pub fn run_refresh_probe_command() -> Result<i32, MarsError> {
    let Some(_lock) = super::probe_lock_blocking(LOCK_FILE) else {
        return Ok(0);
    };

    if let Some(entry) = read_cache_tolerant()
        && is_fresh(&entry)
        && is_usable_result(entry.result.as_ref())
    {
        return Ok(0);
    }

    let probe_result = super::pi::probe();
    if let Ok(path) = cache_path() {
        let existing = read_cache_tolerant();
        persist_probe_attempt(&path, existing.as_ref(), &probe_result);
    }

    Ok(0)
}

fn is_usable_result(result: Option<&PiProbeResult>) -> bool {
    result.is_some_and(|probe| probe.model_probe_success && probe.error.is_none())
}
