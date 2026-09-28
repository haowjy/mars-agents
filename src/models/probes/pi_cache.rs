use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use super::pi::PiProbeResult;
use super::probe_refresh::ProbeCacheBranch;
use crate::error::MarsError;

const SCHEMA_VERSION: u32 = 3;
const DEFAULT_TTL_SECS: u64 = 60;

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

fn cache_dir() -> Result<PathBuf, MarsError> {
    let root = crate::platform::cache::global_cache_root()?;
    Ok(root.join("availability"))
}

fn cache_path() -> Result<PathBuf, MarsError> {
    Ok(cache_dir()?.join("pi.json"))
}

fn lock_path() -> Result<PathBuf, MarsError> {
    Ok(cache_dir()?.join(".pi.lock"))
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

struct FileLock {
    _file: std::fs::File,
}

fn try_lock() -> Option<FileLock> {
    lock_at(&lock_path().ok()?, true)
}

fn blocking_lock() -> Option<FileLock> {
    lock_at(&lock_path().ok()?, false)
}

fn lock_at(path: &Path, nonblocking: bool) -> Option<FileLock> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .ok()?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let flags = if nonblocking {
            libc::LOCK_EX | libc::LOCK_NB
        } else {
            libc::LOCK_EX
        };
        let ret = unsafe { libc::flock(file.as_raw_fd(), flags) };
        if ret != 0 {
            return None;
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Storage::FileSystem::{
            LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
        };
        let handle = file.as_raw_handle() as HANDLE;
        let mut overlapped = unsafe { std::mem::zeroed() };
        let flags = if nonblocking {
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY
        } else {
            LOCKFILE_EXCLUSIVE_LOCK
        };
        let ret = unsafe { LockFileEx(handle, flags, 0, 1, 0, &mut overlapped) };
        if ret == 0 {
            return None;
        }
    }

    Some(FileLock { _file: file })
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
        || spawn_detached_refresh().map_err(|_| ()),
    )
}

#[cfg(test)]
fn probe_cached_impl<F, S>(
    mars_offline: bool,
    probe_refresh: super::ProbeRefreshMode,
    path: &Option<PathBuf>,
    probe: F,
    spawn_refresh: S,
) -> CachedPiProbeOutcome
where
    F: Fn() -> PiProbeResult,
    S: Fn() -> Result<(), ()>,
{
    probe_cached_impl_observed(mars_offline, probe_refresh, path, probe, spawn_refresh).outcome
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
    let observation = if matches!(outcome, CachedPiProbeOutcome::Unavailable) {
        None
    } else {
        observation
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
    let Some(lock) = try_lock() else { return };
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
    let lock = blocking_lock();
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
        let outcome = if probe_result.model_probe_success && probe_result.error.is_none() {
            CachedPiProbeOutcome::Miss(probe_result)
        } else {
            CachedPiProbeOutcome::Failed(probe_result)
        };
        return ObservedOutcome {
            outcome,
            observation: Some(super::ProbeObservation {
                observed_at: Some(now_unix_secs()),
                last_error: None,
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

    let outcome = if probe_result.model_probe_success && probe_result.error.is_none() {
        CachedPiProbeOutcome::Miss(probe_result)
    } else {
        CachedPiProbeOutcome::Failed(probe_result)
    };
    ObservedOutcome {
        outcome,
        observation: Some(super::ProbeObservation {
            observed_at: Some(now_unix_secs()),
            last_error: None,
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

fn spawn_detached_refresh() -> std::io::Result<()> {
    let mars_bin = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(mars_bin);
    cmd.args(["models", "__refresh-probe", "--target", "pi"]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008);
    }

    cmd.spawn()?;
    Ok(())
}

pub fn run_refresh_probe_command() -> Result<i32, MarsError> {
    let Some(_lock) = blocking_lock() else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn compatible_result() -> PiProbeResult {
        PiProbeResult {
            binary_path: "/tmp/pi".to_string(),
            version: Some("pi 0.4.2".to_string()),
            compatible: true,
            model_probe_success: true,
            help_surface_tokens_present: vec!["--mode".to_string()],
            help_surface_tokens_missing: Vec::new(),
            model_slugs: HashSet::from(["openai/gpt-5.4".to_string()]),
            error: None,
        }
    }

    fn incompatible_result() -> PiProbeResult {
        PiProbeResult {
            compatible: false,
            help_surface_tokens_missing: vec!["--mode".to_string()],
            error: Some("missing help tokens".to_string()),
            ..PiProbeResult::default()
        }
    }

    fn entry(fetched_at: u64, result: Option<PiProbeResult>) -> PiProbeCacheEntry {
        PiProbeCacheEntry {
            schema_version: SCHEMA_VERSION,
            harness: "pi".to_string(),
            fetched_at,
            last_attempt_at: fetched_at,
            last_error: None,
            result,
        }
    }

    fn cache_file(temp: &TempDir) -> PathBuf {
        temp.path().join("availability").join("pi.json")
    }

    fn write_entry(path: &Path, entry: &PiProbeCacheEntry) {
        write_cache_at(path, entry).unwrap();
    }

    #[test]
    fn legacy_cache_without_independent_listing_bit_is_reprobed() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "schema_version": 2,
                "harness": "pi",
                "fetched_at": now_unix_secs(),
                "last_attempt_at": now_unix_secs(),
                "last_error": null,
                "result": {
                    "binary_path": "/tmp/pi",
                    "version": "pi 0.4.2",
                    "compatible": true,
                    "help_surface_tokens_present": ["--mode"],
                    "help_surface_tokens_missing": [],
                    "error": null
                }
            })
            .to_string(),
        )
        .unwrap();

        let outcome = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Background,
            &Some(path),
            || PiProbeResult {
                model_slugs: HashSet::from(["openai/gpt-5.5".to_string()]),
                ..compatible_result()
            },
            || Ok(()),
        );

        let CachedPiProbeOutcome::Miss(result) = outcome else {
            panic!("legacy cache entries without model slug capability must trigger a fresh probe");
        };
        assert!(result.model_slugs.contains("openai/gpt-5.5"));
    }

    #[test]
    fn fresh_hit_returns_cached_result() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        write_entry(&path, &entry(now_unix_secs(), Some(compatible_result())));

        let outcome = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Background,
            &Some(path),
            incompatible_result,
            || Ok(()),
        );
        assert!(matches!(outcome, CachedPiProbeOutcome::Hit(_)));
    }

    #[test]
    fn stale_entry_returns_stale_result() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        write_entry(&path, &entry(1, Some(compatible_result())));

        let outcome = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Background,
            &Some(path),
            incompatible_result,
            || Ok(()),
        );
        assert!(matches!(outcome, CachedPiProbeOutcome::Stale(_)));
    }

    #[test]
    fn failed_pi_probe_keeps_last_good_listing() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        write_entry(&path, &entry(1, Some(compatible_result())));
        let outcome = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Synchronous,
            &Some(path.clone()),
            incompatible_result,
            || Ok(()),
        );
        assert!(matches!(outcome, CachedPiProbeOutcome::StaleFailed(_)));
        assert!(!outcome.latest_attempt_ok());
        assert!(
            outcome
                .result()
                .unwrap()
                .model_slugs
                .contains("openai/gpt-5.4")
        );
        let on_disk = read_cache_tolerant_at(&path).unwrap();
        assert_eq!(on_disk.fetched_at, 1);
        assert!(on_disk.last_attempt_at > on_disk.fetched_at);
        assert!(
            on_disk
                .result
                .unwrap()
                .model_slugs
                .contains("openai/gpt-5.4")
        );
        let offline = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Skip,
            &Some(path),
            || panic!("skip must not re-probe"),
            || Ok(()),
        );
        assert!(!offline.latest_attempt_ok());
        assert!(
            offline
                .result()
                .unwrap()
                .model_slugs
                .contains("openai/gpt-5.4")
        );
    }

    #[test]
    fn detached_refresh_write_decision_preserves_last_good_listing() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        let previous = entry(1, Some(compatible_result()));
        write_entry(&path, &previous);

        let retained = persist_probe_attempt(&path, Some(&previous), &incompatible_result());
        assert_eq!(retained, previous.result);
        let on_disk = read_cache_tolerant_at(&path).unwrap();
        assert_eq!(on_disk.fetched_at, 1);
        assert!(on_disk.last_attempt_at > on_disk.fetched_at);
        assert_eq!(on_disk.result, retained);
        assert!(on_disk.last_error.is_some());
    }

    #[test]
    fn missing_cache_runs_probe() {
        let temp = TempDir::new().unwrap();
        let path = cache_file(&temp);
        let outcome = probe_cached_impl(
            false,
            crate::models::probes::ProbeRefreshMode::Background,
            &Some(path),
            compatible_result,
            || Ok(()),
        );
        assert!(matches!(outcome, CachedPiProbeOutcome::Miss(_)));
    }
}
