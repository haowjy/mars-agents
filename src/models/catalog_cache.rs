//! Last-known-good models.dev snapshot and stale-while-revalidate lifecycle.

use super::catalog_api::{
    CATALOG_HTTP_DEADLINE_SECS, CachedModel, default_catalog_providers, fetch_models_with_providers,
};
use crate::error::MarsError;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

mod tracing {
    macro_rules! debug { ($($arg:tt)*) => { if cfg!(debug_assertions) { eprintln!($($arg)*); } }; }
    pub(super) use debug;
}

/// Cached model catalog from external API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCache {
    pub models: Vec<CachedModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
}

/// One durable, atomically replaced catalog record. Legacy snapshots without
/// `revision` deserialize as revision zero; revision is never a model field.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    #[serde(flatten)]
    cache: ModelsCache,
    #[serde(default)]
    revision: u64,
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            cache: ModelsCache {
                models: Vec::new(),
                fetched_at: None,
            },
            revision: 0,
        }
    }
}

const CACHE_FILE: &str = "models-cache.json";
const FETCH_FAIL_MARKER_FILE: &str = ".models-cache.last-fail";
const REFRESH_CLAIM_FILE: &str = ".models-cache.refresh-claim";
const REFRESH_CLAIM_LOCK_FILE: &str = ".models-cache.refresh-claim.lock";
const REFRESH_CLAIM_LEASE_SECS: u64 = 120;
const _: () = assert!(CATALOG_HTTP_DEADLINE_SECS < REFRESH_CLAIM_LEASE_SECS);
pub(crate) const FETCH_FAIL_COOLDOWN_SECS: u64 = 300;
const FETCH_FAIL_COOLDOWN_REASON: &str = "recent fetch attempt failed; backing off (cooldown)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshMode {
    /// Return stale usable data immediately and refresh in a detached worker.
    Background,
    /// Refresh stale data in this process (used by the internal worker).
    Synchronous,
    Force,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackgroundRefresh {
    Spawned,
    AlreadyInProgress,
    Cooldown,
    SpawnFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RefreshOutcome {
    AlreadyFresh,
    PeerRefreshed,
    Refreshed {
        models_count: usize,
    },
    Stale {
        refresh: BackgroundRefresh,
        last_failure: Option<String>,
    },
    StaleFallback {
        reason: String,
    },
    Offline,
}

pub fn refresh_warning(outcome: &RefreshOutcome) -> Option<String> {
    match outcome {
        RefreshOutcome::Stale {
            refresh,
            last_failure,
        } => {
            let status = match refresh {
                BackgroundRefresh::Spawned => "background refresh started".to_string(),
                BackgroundRefresh::AlreadyInProgress => {
                    "background refresh already in progress".to_string()
                }
                BackgroundRefresh::Cooldown => {
                    "background refresh suppressed by cooldown".to_string()
                }
                BackgroundRefresh::SpawnFailed { reason } => {
                    format!("background refresh failed to spawn: {reason}")
                }
            };
            let previous = last_failure
                .as_ref()
                .map(|reason| format!("; previous refresh failed: {reason}"))
                .unwrap_or_default();
            Some(format!("using stale models cache; {status}{previous}"))
        }
        RefreshOutcome::StaleFallback { reason } => Some(format!(
            "models cache refresh failed: {reason}; using stale cache"
        )),
        _ => None,
    }
}

pub fn now_unix_secs_value() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_unix_secs() -> String {
    now_unix_secs_value().to_string()
}

pub fn is_mars_offline() -> bool {
    match std::env::var("MARS_OFFLINE") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        ),
        Err(_) => false,
    }
}
fn read_snapshot_tolerant(mars_dir: &Path) -> Snapshot {
    match read_snapshot(mars_dir) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            tracing::debug!("models cache read failed, treating as empty: {err}");
            Snapshot::empty()
        }
    }
}

fn is_fresh(cache: &ModelsCache, ttl_hours: u32) -> bool {
    if ttl_hours == 0 {
        return false;
    }
    if cache.models.is_empty() {
        return false;
    }

    let Some(fetched_str) = &cache.fetched_at else {
        return false;
    };
    let Ok(fetched) = fetched_str.parse::<u64>() else {
        return false;
    };

    let now = now_unix_secs_value();
    if fetched > now {
        return false;
    }

    (now - fetched) < (ttl_hours as u64) * 3600
}

fn is_usable(cache: &ModelsCache) -> bool {
    !cache.models.is_empty()
}

#[derive(Serialize, Deserialize)]
struct FetchFailure {
    at: u64,
    reason: String,
}

fn read_fetch_fail_marker(mars_dir: &Path) -> Option<FetchFailure> {
    let marker = mars_dir.join(FETCH_FAIL_MARKER_FILE);
    let raw = std::fs::read_to_string(marker).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_fetch_fail_marker(mars_dir: &Path, reason: &str) {
    let marker = mars_dir.join(FETCH_FAIL_MARKER_FILE);
    let failure = FetchFailure {
        at: now_unix_secs_value(),
        reason: reason.to_string(),
    };
    let content = serde_json::to_vec(&failure).expect("fetch failure is serializable");
    if let Err(err) = crate::fs::atomic_write(&marker, &content) {
        tracing::debug!("failed to write models fetch failure marker: {err}");
    }
}

fn clear_fetch_fail_marker(mars_dir: &Path) {
    let marker = mars_dir.join(FETCH_FAIL_MARKER_FILE);
    if let Err(err) = std::fs::remove_file(marker)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!("failed to clear models fetch failure marker: {err}");
    }
}

#[derive(Serialize, Deserialize)]
struct RefreshClaim {
    token: String,
    at: u64,
}

fn read_refresh_claim(mars_dir: &Path) -> Result<Option<RefreshClaim>, MarsError> {
    let path = mars_dir.join(REFRESH_CLAIM_FILE);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // An invalid record cannot identify a live owner. Atomic replacement
    // prevents partial new records, but also permits recovery from corruption.
    Ok(serde_json::from_slice(&bytes).ok())
}

enum RefreshClaimDecision {
    Claimed {
        token: String,
        cache: ModelsCache,
        last_failure: Option<String>,
    },
    Suppressed {
        cache: ModelsCache,
        outcome: RefreshOutcome,
    },
}

/// Cohesive refresh context: policy and provider scope travel together.
/// Snapshot reads/writes, claim lifecycle and worker dispatch are owned here.
struct CatalogCache<'a> {
    mars_dir: &'a Path,
    ttl_hours: u32,
    providers: &'a [String],
}

impl<'a> CatalogCache<'a> {
    fn new(mars_dir: &'a Path, ttl_hours: u32, providers: &'a [String]) -> Self {
        Self {
            mars_dir,
            ttl_hours,
            providers,
        }
    }
}

impl CatalogCache<'_> {
    fn claim_refresh(&self, observed_revision: u64) -> Result<RefreshClaimDecision, MarsError> {
        let mars_dir = self.mars_dir;
        let ttl_hours = self.ttl_hours;
        let _lock = crate::fs::FileLock::acquire(&mars_dir.join(REFRESH_CLAIM_LOCK_FILE))?;
        // The worker holds its claim until snapshot and failure marker
        // updates are complete. Rechecking here closes the read-to-claim race.
        let snapshot = read_snapshot_tolerant(mars_dir);
        let current_revision = snapshot.revision;
        let cache = snapshot.cache;
        let failure = read_fetch_fail_marker(mars_dir);
        let last_failure = failure.as_ref().map(|failure| failure.reason.clone());
        let now = now_unix_secs_value();
        if read_refresh_claim(mars_dir)?
            .is_some_and(|claim| claim.at <= now && now - claim.at < REFRESH_CLAIM_LEASE_SECS)
        {
            let outcome = if is_fresh(&cache, ttl_hours) {
                RefreshOutcome::AlreadyFresh
            } else {
                RefreshOutcome::Stale {
                    refresh: BackgroundRefresh::AlreadyInProgress,
                    last_failure,
                }
            };
            return Ok(RefreshClaimDecision::Suppressed { cache, outcome });
        }
        if failure
            .as_ref()
            .is_some_and(|failure| now.saturating_sub(failure.at) < FETCH_FAIL_COOLDOWN_SECS)
        {
            return Ok(RefreshClaimDecision::Suppressed {
                cache,
                outcome: RefreshOutcome::Stale {
                    refresh: BackgroundRefresh::Cooldown,
                    last_failure,
                },
            });
        }
        if current_revision != observed_revision && is_usable(&cache) {
            return Ok(RefreshClaimDecision::Suppressed {
                cache,
                outcome: RefreshOutcome::PeerRefreshed,
            });
        }
        if is_fresh(&cache, ttl_hours) {
            return Ok(RefreshClaimDecision::Suppressed {
                cache,
                outcome: RefreshOutcome::AlreadyFresh,
            });
        }
        // tempfile supplies a cross-process unique token without an extra
        // dependency. Only the durable claim record remains after this scope.
        let token_file = tempfile::Builder::new()
            .prefix(".models-claim-token-")
            .tempfile_in(mars_dir)?;
        let token = token_file
            .path()
            .file_name()
            .expect("tempfile has a filename")
            .to_string_lossy()
            .into_owned();
        let claim = RefreshClaim {
            token: token.clone(),
            at: now,
        };
        crate::fs::atomic_write(
            &mars_dir.join(REFRESH_CLAIM_FILE),
            &serde_json::to_vec(&claim).expect("refresh claim is serializable"),
        )?;
        Ok(RefreshClaimDecision::Claimed {
            token,
            cache,
            last_failure,
        })
    }
}

fn refresh_claim_is_owned(mars_dir: &Path, token: &str) -> Result<bool, MarsError> {
    let _lock = crate::fs::FileLock::acquire(&mars_dir.join(REFRESH_CLAIM_LOCK_FILE))?;
    Ok(read_refresh_claim(mars_dir)?.is_some_and(|claim| claim.token == token))
}

fn release_refresh_claim(mars_dir: &Path, token: &str) -> Result<(), MarsError> {
    let _lock = crate::fs::FileLock::acquire(&mars_dir.join(REFRESH_CLAIM_LOCK_FILE))?;
    if read_refresh_claim(mars_dir)?.is_some_and(|claim| claim.token == token) {
        std::fs::remove_file(mars_dir.join(REFRESH_CLAIM_FILE))?;
    }
    Ok(())
}

struct RefreshClaimGuard<'a> {
    mars_dir: &'a Path,
    token: &'a str,
}

impl Drop for RefreshClaimGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = release_refresh_claim(self.mars_dir, self.token) {
            tracing::debug!("failed to clear catalog refresh claim: {error}");
        }
    }
}

pub fn ensure_fresh(
    mars_dir: &Path,
    ttl_hours: u32,
    mode: RefreshMode,
) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
    ensure_fresh_with_catalog_providers(mars_dir, ttl_hours, mode, &default_catalog_providers())
}

pub fn ensure_fresh_with_catalog_providers(
    mars_dir: &Path,
    ttl_hours: u32,
    mode: RefreshMode,
    providers: &[String],
) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
    CatalogCache::new(mars_dir, ttl_hours, providers).ensure(mode)
}

impl CatalogCache<'_> {
    fn ensure(&self, mode: RefreshMode) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
        let mars_dir = self.mars_dir;
        let ttl_hours = self.ttl_hours;
        let providers = self.providers;
        if mode == RefreshMode::Background && !is_mars_offline() {
            std::fs::create_dir_all(mars_dir)?;
            let snapshot = read_snapshot_tolerant(mars_dir);
            let observed_revision = snapshot.revision;
            let prior = snapshot.cache;
            if is_usable(&prior) {
                if is_fresh(&prior, ttl_hours) {
                    return Ok((prior, RefreshOutcome::AlreadyFresh));
                }
                return Ok(
                    self.schedule_background_refresh(observed_revision, prior, |token| {
                        self.spawn_background_refresh(observed_revision, token)
                    }),
                );
            }
        }
        self.refresh_with_fetcher(mode, None, move || fetch_models_with_providers(providers))
    }
}

impl CatalogCache<'_> {
    fn schedule_background_refresh(
        &self,
        observed_revision: u64,
        prior: ModelsCache,
        spawn: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> (ModelsCache, RefreshOutcome) {
        let mars_dir = self.mars_dir;
        match self.claim_refresh(observed_revision) {
            Ok(RefreshClaimDecision::Suppressed { cache, outcome }) => (cache, outcome),
            Ok(RefreshClaimDecision::Claimed {
                token,
                cache,
                last_failure,
            }) => {
                let refresh = match spawn(&token) {
                    Ok(()) => BackgroundRefresh::Spawned,
                    Err(error) => {
                        let mut reason = error.to_string();
                        if let Err(release_error) = release_refresh_claim(mars_dir, &token) {
                            reason.push_str(&format!(
                                "; failed to clear refresh claim: {release_error}"
                            ));
                        }
                        BackgroundRefresh::SpawnFailed { reason }
                    }
                };
                (
                    cache,
                    RefreshOutcome::Stale {
                        refresh,
                        last_failure,
                    },
                )
            }
            Err(error) => (
                prior,
                RefreshOutcome::Stale {
                    refresh: BackgroundRefresh::SpawnFailed {
                        reason: error.to_string(),
                    },
                    last_failure: read_fetch_fail_marker(mars_dir).map(|failure| failure.reason),
                },
            ),
        }
    }
}

impl CatalogCache<'_> {
    fn spawn_background_refresh(
        &self,
        expected_revision: u64,
        claim_token: &str,
    ) -> std::io::Result<()> {
        let mars_dir = self.mars_dir;
        let ttl_hours = self.ttl_hours;
        let providers = self.providers;
        let program = std::env::current_exe()?;
        let mut args = vec![
            OsString::from("--root"),
            mars_dir
                .parent()
                .unwrap_or(mars_dir)
                .as_os_str()
                .to_os_string(),
            OsString::from("models"),
            OsString::from("__refresh-catalog"),
            OsString::from("--mars-dir"),
            mars_dir.as_os_str().to_os_string(),
            OsString::from("--refresh-after-hours"),
            OsString::from(ttl_hours.to_string()),
        ];
        for provider in providers {
            args.push(OsString::from("--provider"));
            args.push(OsString::from(provider));
        }
        args.extend([
            OsString::from("--expected-revision"),
            OsString::from(expected_revision.to_string()),
            OsString::from("--claim-token"),
            OsString::from(claim_token),
        ]);
        crate::platform::process::spawn_detached(program.as_os_str(), &args)
    }
}

/// Entry point for the hidden worker command. Never selects Background, so it cannot recurse.
pub fn run_background_refresh(
    mars_dir: &Path,
    ttl_hours: u32,
    providers: &[String],
    expected_revision: u64,
    claim_token: &str,
) -> Result<(), MarsError> {
    CatalogCache::new(mars_dir, ttl_hours, providers).run_worker(expected_revision, claim_token)
}

impl CatalogCache<'_> {
    fn run_worker(&self, expected_revision: u64, claim_token: &str) -> Result<(), MarsError> {
        let mars_dir = self.mars_dir;
        let _claim = RefreshClaimGuard {
            mars_dir,
            token: claim_token,
        };
        if !refresh_claim_is_owned(mars_dir, claim_token)? {
            return Ok(());
        }
        self.refresh_with_fetcher(RefreshMode::Synchronous, Some(expected_revision), || {
            fetch_models_with_providers(self.providers)
        })?;
        Ok(())
    }
}

impl CatalogCache<'_> {
    fn refresh_with_fetcher<F>(
        &self,
        mode: RefreshMode,
        expected_revision: Option<u64>,
        fetcher: F,
    ) -> Result<(ModelsCache, RefreshOutcome), MarsError>
    where
        F: FnOnce() -> Result<Vec<CachedModel>, MarsError>,
    {
        let mars_dir = self.mars_dir;
        let ttl_hours = self.ttl_hours;
        std::fs::create_dir_all(mars_dir)?;

        // D1: apply MARS_OFFLINE coercion exactly once here.
        let effective_mode = if is_mars_offline() {
            RefreshMode::Offline
        } else {
            mode
        };

        let prior = read_snapshot_tolerant(mars_dir).cache;

        if matches!(
            effective_mode,
            RefreshMode::Background | RefreshMode::Synchronous
        ) && is_fresh(&prior, ttl_hours)
        {
            return Ok((prior, RefreshOutcome::AlreadyFresh));
        }

        if effective_mode == RefreshMode::Offline {
            if is_usable(&prior) {
                return Ok((prior, RefreshOutcome::Offline));
            }
            return Err(MarsError::ModelCacheUnavailable {
                reason: offline_unavailable_reason(mode),
            });
        }

        let lock_path = mars_dir.join(".models-cache.lock");
        let _guard = crate::fs::FileLock::acquire(&lock_path)?;

        let snapshot = read_snapshot_tolerant(mars_dir);
        let under_lock = snapshot.cache;
        if expected_revision.is_some_and(|expected| snapshot.revision != expected) {
            return Ok((under_lock, RefreshOutcome::AlreadyFresh));
        }
        if matches!(
            effective_mode,
            RefreshMode::Background | RefreshMode::Synchronous
        ) && is_fresh(&under_lock, ttl_hours)
        {
            return Ok((under_lock, RefreshOutcome::AlreadyFresh));
        }

        if mode != RefreshMode::Force && is_usable(&under_lock) {
            let now = now_unix_secs_value();
            if let Some(last_fail) = read_fetch_fail_marker(mars_dir)
                && now.saturating_sub(last_fail.at) < FETCH_FAIL_COOLDOWN_SECS
            {
                return Ok((
                    under_lock,
                    RefreshOutcome::StaleFallback {
                        reason: FETCH_FAIL_COOLDOWN_REASON.to_string(),
                    },
                ));
            }
        }

        match fetcher() {
            Ok(models) if !models.is_empty() => {
                let models_count = models.len();
                let revision = snapshot.revision.checked_add(1).ok_or_else(|| {
                    MarsError::Config(crate::error::ConfigError::Invalid {
                        message: "catalog snapshot revision exhausted".to_string(),
                    })
                })?;
                let cache = ModelsCache {
                    models,
                    fetched_at: Some(now_unix_secs()),
                };
                write_snapshot(
                    mars_dir,
                    &Snapshot {
                        cache: cache.clone(),
                        revision,
                    },
                )?;
                clear_fetch_fail_marker(mars_dir);
                Ok((cache, RefreshOutcome::Refreshed { models_count }))
            }
            Ok(_) => fallback_to_stale_or_error(
                mars_dir,
                under_lock,
                "API returned empty catalog".to_string(),
                "API returned an empty catalog and no prior cache exists".to_string(),
            ),
            Err(err) => fallback_to_stale_or_error(
                mars_dir,
                under_lock,
                format!("fetch failed: {err}"),
                format!("automatic refresh failed: {err}"),
            ),
        }
    }
}

#[cfg(test)]
fn ensure_fresh_with_fetcher<F>(
    mars_dir: &Path,
    ttl_hours: u32,
    mode: RefreshMode,
    fetcher: F,
) -> Result<(ModelsCache, RefreshOutcome), MarsError>
where
    F: FnOnce() -> Result<Vec<CachedModel>, MarsError>,
{
    CatalogCache::new(mars_dir, ttl_hours, &[]).refresh_with_fetcher(mode, None, fetcher)
}

#[cfg(test)]
fn ensure_fresh_with_fetcher_if_revision<F>(
    mars_dir: &Path,
    ttl_hours: u32,
    mode: RefreshMode,
    expected_revision: Option<u64>,
    fetcher: F,
) -> Result<(ModelsCache, RefreshOutcome), MarsError>
where
    F: FnOnce() -> Result<Vec<CachedModel>, MarsError>,
{
    CatalogCache::new(mars_dir, ttl_hours, &[]).refresh_with_fetcher(
        mode,
        expected_revision,
        fetcher,
    )
}

fn fallback_to_stale_or_error(
    mars_dir: &Path,
    under_lock: ModelsCache,
    stale_reason: String,
    unavailable_reason: String,
) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
    if is_usable(&under_lock) {
        write_fetch_fail_marker(mars_dir, &stale_reason);
        Ok((
            under_lock,
            RefreshOutcome::StaleFallback {
                reason: stale_reason,
            },
        ))
    } else {
        Err(MarsError::ModelCacheUnavailable {
            reason: unavailable_reason,
        })
    }
}

fn offline_unavailable_reason(requested_mode: RefreshMode) -> String {
    match requested_mode {
        RefreshMode::Offline => {
            "--no-refresh-models was passed and no cached catalog is available".to_string()
        }
        RefreshMode::Background | RefreshMode::Synchronous | RefreshMode::Force => {
            "MARS_OFFLINE is set and no cached catalog is available".to_string()
        }
    }
}

/// Read models cache from `.mars/models-cache.json`.
pub fn read_cache(mars_dir: &Path) -> Result<ModelsCache, MarsError> {
    read_snapshot(mars_dir).map(|snapshot| snapshot.cache)
}

fn read_snapshot(mars_dir: &Path) -> Result<Snapshot, MarsError> {
    let path = mars_dir.join(CACHE_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let snapshot: Snapshot =
                serde_json::from_str(&content).map_err(|e| crate::error::ConfigError::Invalid {
                    message: format!("failed to parse models cache: {e}"),
                })?;
            Ok(snapshot)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot::empty()),
        Err(source) => Err(MarsError::Io {
            operation: "read models cache".to_string(),
            path,
            source,
        }),
    }
}

fn write_snapshot(mars_dir: &Path, snapshot: &Snapshot) -> Result<(), MarsError> {
    std::fs::create_dir_all(mars_dir)?;
    let path = mars_dir.join(CACHE_FILE);
    let content =
        serde_json::to_string_pretty(snapshot).map_err(|e| crate::error::ConfigError::Invalid {
            message: format!("failed to serialize models cache: {e}"),
        })?;
    crate::fs::atomic_write(&path, content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
fn write_cache(mars_dir: &Path, cache: &ModelsCache) -> Result<(), MarsError> {
    write_snapshot(
        mars_dir,
        &Snapshot {
            cache: cache.clone(),
            revision: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::resolve_models_refresh_control;
    use httpmock::prelude::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn legacy_snapshot_defaults_to_zero_and_revision_is_not_a_model_field() {
        let mars = tempdir().unwrap();
        std::fs::write(
            mars.path().join(CACHE_FILE),
            r#"{"models":[{"id":"old","provider":"OpenAI"}],"fetched_at":"1"}"#,
        )
        .unwrap();
        std::fs::write(mars.path().join(".models-cache.generation"), "999").unwrap();

        let old = read_snapshot(mars.path()).unwrap();
        assert_eq!(old.revision, 0, "legacy sidecar is never authoritative");
        assert_eq!(old.cache.models[0].id, "old");

        let (cache, outcome) = ensure_fresh_with_fetcher_if_revision(
            mars.path(),
            0,
            RefreshMode::Synchronous,
            Some(0),
            || Ok(vec![sample_cached_model("new")]),
        )
        .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(cache.models[0].id, "new");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(mars.path().join(CACHE_FILE)).unwrap()).unwrap();
        assert_eq!(raw["revision"], 1);
        assert!(raw["models"][0].get("revision").is_none());
        assert!(
            serde_json::to_value(read_cache(mars.path()).unwrap())
                .unwrap()
                .get("revision")
                .is_none()
        );
    }

    #[test]
    fn failed_and_empty_fetches_never_replace_snapshot_or_advance_revision() {
        let mars = tempdir().unwrap();
        write_snapshot(
            mars.path(),
            &Snapshot {
                cache: ModelsCache {
                    models: vec![sample_cached_model("last-good")],
                    fetched_at: Some("1".to_string()),
                },
                revision: 7,
            },
        )
        .unwrap();
        let before = std::fs::read(mars.path().join(CACHE_FILE)).unwrap();
        for fetch in [false, true] {
            let (_, outcome) = ensure_fresh_with_fetcher_if_revision(
                mars.path(),
                0,
                RefreshMode::Force,
                None,
                || {
                    if fetch {
                        Ok(Vec::new())
                    } else {
                        Err(MarsError::ModelCacheUnavailable {
                            reason: "network down".into(),
                        })
                    }
                },
            )
            .unwrap();
            assert!(matches!(outcome, RefreshOutcome::StaleFallback { .. }));
            assert_eq!(std::fs::read(mars.path().join(CACHE_FILE)).unwrap(), before);
            assert_eq!(read_snapshot(mars.path()).unwrap().revision, 7);
        }
    }
    #[allow(unused_unsafe)]
    fn env_set(key: &str, value: &str) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    #[allow(unused_unsafe)]
    fn env_remove(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    struct EnvVarGuard {
        key: String,
        prev: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            env_set(key, value);
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                env_set(&self.key, prev);
            } else {
                env_remove(&self.key);
            }
        }
    }

    fn sample_catalog_json() -> serde_json::Value {
        serde_json::json!({
            "openai": {
                "models": {
                    "gpt-5": {
                        "id": "gpt-5",
                        "name": "GPT-5",
                        "release_date": "2025-06-01",
                        "limit": {
                            "context": 400000,
                            "output": 128000
                        }
                    }
                }
            },
            "anthropic": {
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "release_date": "2025-03-01"
                    }
                }
            }
        })
    }

    fn sample_cached_model(id: &str) -> CachedModel {
        CachedModel {
            id: id.to_string(),
            provider: "OpenAI".to_string(),
            release_date: None,
            description: None,
            context_window: None,
            max_output: None,
            cost_input: None,
            cost_output: None,
            cost_cache_read: None,
            cost_cache_write: None,
            cost_reasoning: None,
        }
    }

    fn write_cache_state(mars_dir: &std::path::Path, models: Vec<CachedModel>, fetched_at: &str) {
        write_cache(
            mars_dir,
            &ModelsCache {
                models,
                fetched_at: Some(fetched_at.to_string()),
            },
        )
        .expect("failed to write cache fixture");
    }

    fn write_raw_cache_file(mars_dir: &std::path::Path, raw: &str) {
        std::fs::create_dir_all(mars_dir).expect("failed to create mars dir");
        std::fs::write(mars_dir.join(CACHE_FILE), raw).expect("failed to write raw cache");
    }

    fn stale_timestamp() -> String {
        now_unix_secs_value().saturating_sub(48 * 3600).to_string()
    }

    fn fresh_timestamp() -> String {
        now_unix_secs_value().saturating_sub(60).to_string()
    }

    fn assert_model_cache_unavailable(
        result: Result<(ModelsCache, RefreshOutcome), MarsError>,
        reason_contains: &str,
    ) {
        match result {
            Err(MarsError::ModelCacheUnavailable { reason }) => {
                assert!(
                    reason.contains(reason_contains),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected ModelCacheUnavailable, got {other:?}"),
        }
    }

    #[test]
    #[serial]
    fn ensure_fresh_1_missing_cache_offline_errors() {
        let mars = tempdir().unwrap();
        let _offline = EnvVarGuard::set("MARS_OFFLINE", "1");

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous);
        assert_model_cache_unavailable(result, "MARS_OFFLINE is set");
    }

    #[test]
    #[serial]
    fn ensure_fresh_2_missing_cache_auto_fetch_failure_errors() {
        let mars = tempdir().unwrap();
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(500).body("server error");
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous);
        assert_model_cache_unavailable(result, "automatic refresh failed");
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    fn ensure_fresh_3_stale_usable_offline_returns_stale() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Offline).unwrap();
        assert_eq!(cache.models.len(), 1);
        assert_eq!(cache.models[0].id, "stale-model");
        assert_eq!(outcome, RefreshOutcome::Offline);
    }

    #[test]
    #[serial]
    fn ensure_fresh_4_fresh_auto_skips_http() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("fresh-model")],
            &fresh_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (_cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert_eq!(outcome, RefreshOutcome::AlreadyFresh);
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    #[serial]
    fn ensure_fresh_5_stale_auto_success_refreshes() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &stale_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert!(matches!(
            outcome,
            RefreshOutcome::Refreshed { models_count } if models_count == 2
        ));
        assert_eq!(cache.models.len(), 2);
        assert!(!cache.models.is_empty());
        assert!(cache.fetched_at.is_some());
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_6_stale_auto_fetch_failure_falls_back() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(500).body("server error");
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert_eq!(cache.models[0].id, "stale-model");
        assert!(matches!(
            outcome,
            RefreshOutcome::StaleFallback { reason } if reason.contains("fetch failed")
        ));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_7_stale_auto_empty_catalog_falls_back() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(serde_json::json!({}));
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert_eq!(cache.models[0].id, "stale-model");
        assert!(matches!(
            outcome,
            RefreshOutcome::StaleFallback { reason } if reason == "API returned empty catalog"
        ));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_8_empty_cache_auto_refetches() {
        let mars = tempdir().unwrap();
        write_cache_state(mars.path(), Vec::new(), &fresh_timestamp());

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert!(!cache.models.is_empty());
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    fn ensure_fresh_9_empty_cache_offline_errors() {
        let mars = tempdir().unwrap();
        write_cache_state(mars.path(), Vec::new(), &fresh_timestamp());

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Offline);
        assert_model_cache_unavailable(result, "--no-refresh-models was passed");
    }

    #[test]
    #[serial]
    fn ensure_fresh_10_corrupt_json_auto_refetches() {
        let mars = tempdir().unwrap();
        write_raw_cache_file(mars.path(), "{ not-json ");

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert!(!cache.models.is_empty());
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    fn ensure_fresh_11_corrupt_json_offline_errors() {
        let mars = tempdir().unwrap();
        write_raw_cache_file(mars.path(), "{ not-json ");

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Offline);
        assert_model_cache_unavailable(result, "--no-refresh-models was passed");
    }

    #[test]
    fn read_cache_io_error_includes_operation_and_path() {
        let mars = tempdir().unwrap();
        let cache_path = mars.path().join(CACHE_FILE);
        std::fs::create_dir(&cache_path).unwrap();

        let err = read_cache(mars.path()).unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("read models cache"),
            "error should include operation context: {msg}"
        );
        assert!(
            msg.contains(CACHE_FILE),
            "error should include cache path: {msg}"
        );
    }

    #[test]
    #[serial]
    fn ensure_fresh_12_ttl_zero_always_refetches() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("fresh-model")],
            &fresh_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (_cache, outcome) = ensure_fresh(mars.path(), 0, RefreshMode::Synchronous).unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_13_unparseable_fetched_at_is_stale() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            "not-a-timestamp",
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (_cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_14_future_fetched_at_is_stale() {
        let mars = tempdir().unwrap();
        let future = now_unix_secs_value() + 3600;
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("future-model")],
            &future.to_string(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let (_cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(mock.hits(), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_15_offline_env_auto_fresh_returns_offline() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("fresh-model")],
            &fresh_timestamp(),
        );

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));
        let _offline = EnvVarGuard::set("MARS_OFFLINE", "1");

        let (_cache, outcome) = ensure_fresh(mars.path(), 24, RefreshMode::Synchronous).unwrap();
        assert_eq!(outcome, RefreshOutcome::Offline);
        assert_eq!(mock.hits(), 0);
    }
    #[test]
    fn resolve_models_refresh_control_defaults_to_auto_background() {
        let control = resolve_models_refresh_control(false, false).unwrap();
        assert_eq!(control.catalog_mode, RefreshMode::Background);
        assert_eq!(
            control.probe_refresh,
            crate::models::probes::ProbeRefreshMode::Background
        );
    }

    #[test]
    fn resolve_models_refresh_control_no_refresh_is_offline_skip() {
        let control = resolve_models_refresh_control(false, true).unwrap();
        assert_eq!(control.catalog_mode, RefreshMode::Offline);
        assert_eq!(
            control.probe_refresh,
            crate::models::probes::ProbeRefreshMode::Skip
        );
    }

    #[test]
    fn resolve_models_refresh_control_refresh_is_force_sync() {
        let control = resolve_models_refresh_control(true, false).unwrap();
        assert_eq!(control.catalog_mode, RefreshMode::Force);
        assert_eq!(
            control.probe_refresh,
            crate::models::probes::ProbeRefreshMode::Synchronous
        );
    }

    #[test]
    fn resolve_models_refresh_control_rejects_both_flags() {
        assert!(resolve_models_refresh_control(true, true).is_err());
    }

    #[test]
    fn refresh_claim_recovers_expired_owner_and_preserves_new_owner() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &stale_timestamp(),
        );
        let observed_revision = read_snapshot(mars.path()).unwrap().revision;
        let old = RefreshClaim {
            token: "crashed-worker".to_string(),
            at: now_unix_secs_value() - REFRESH_CLAIM_LEASE_SECS,
        };
        crate::fs::atomic_write(
            &mars.path().join(REFRESH_CLAIM_FILE),
            &serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();

        let RefreshClaimDecision::Claimed {
            token: new_token, ..
        } = CatalogCache::new(mars.path(), 0, &[])
            .claim_refresh(observed_revision)
            .unwrap()
        else {
            panic!("expired lease should be recovered")
        };
        assert_ne!(new_token, old.token);
        assert!(matches!(
            CatalogCache::new(mars.path(), 0, &[])
                .claim_refresh(observed_revision)
                .unwrap(),
            RefreshClaimDecision::Suppressed {
                outcome: RefreshOutcome::Stale {
                    refresh: BackgroundRefresh::AlreadyInProgress,
                    ..
                },
                ..
            }
        ));
        release_refresh_claim(mars.path(), &old.token).unwrap();
        assert!(refresh_claim_is_owned(mars.path(), &new_token).unwrap());
        release_refresh_claim(mars.path(), &new_token).unwrap();
        assert!(!mars.path().join(REFRESH_CLAIM_FILE).exists());
    }

    #[test]
    fn failed_worker_spawn_releases_refresh_claim() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &stale_timestamp(),
        );
        let observed_revision = read_snapshot(mars.path()).unwrap().revision;
        let (cache, outcome) = CatalogCache::new(mars.path(), 0, &[]).schedule_background_refresh(
            observed_revision,
            read_snapshot_tolerant(mars.path()).cache,
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "worker executable missing",
                ))
            },
        );
        assert!(is_usable(&cache));
        assert!(matches!(
            outcome,
            RefreshOutcome::Stale {
                refresh: BackgroundRefresh::SpawnFailed { .. },
                ..
            }
        ));
        assert!(!mars.path().join(REFRESH_CLAIM_FILE).exists());
        assert!(matches!(
            CatalogCache::new(mars.path(), 0, &[])
                .claim_refresh(observed_revision)
                .unwrap(),
            RefreshClaimDecision::Claimed { .. }
        ));
    }

    #[test]
    fn paused_stale_reader_does_not_spawn_after_peer_success_with_zero_ttl() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &stale_timestamp(),
        );
        let observed_revision = read_snapshot(mars.path()).unwrap().revision;
        let observed_cache = read_snapshot_tolerant(mars.path()).cache;

        let (updated, peer_outcome) = ensure_fresh_with_fetcher_if_revision(
            mars.path(),
            0,
            RefreshMode::Synchronous,
            Some(observed_revision),
            || Ok(vec![sample_cached_model("new-model")]),
        )
        .unwrap();
        assert!(matches!(peer_outcome, RefreshOutcome::Refreshed { .. }));

        let mut launches = 0;
        let (cache, outcome) = CatalogCache::new(mars.path(), 0, &[]).schedule_background_refresh(
            observed_revision,
            observed_cache.clone(),
            |_| {
                launches += 1;
                Ok(())
            },
        );
        assert_ne!(observed_cache.models[0].id, updated.models[0].id);
        assert_eq!(launches, 0, "a completed peer must suppress worker launch");
        assert_eq!(cache.models[0].id, updated.models[0].id);
        assert_eq!(outcome, RefreshOutcome::PeerRefreshed);
    }

    #[test]
    fn paused_stale_reader_does_not_spawn_after_peer_failure_with_zero_ttl() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &stale_timestamp(),
        );
        let observed_revision = read_snapshot(mars.path()).unwrap().revision;
        let observed_cache = read_snapshot_tolerant(mars.path()).cache;

        let (_, peer_outcome) = ensure_fresh_with_fetcher_if_revision(
            mars.path(),
            0,
            RefreshMode::Synchronous,
            Some(observed_revision),
            || {
                Err(MarsError::Http {
                    url: "test".to_string(),
                    status: 0,
                    message: "peer network failure".to_string(),
                })
            },
        )
        .unwrap();
        assert!(matches!(peer_outcome, RefreshOutcome::StaleFallback { .. }));

        let mut launches = 0;
        let (cache, outcome) = CatalogCache::new(mars.path(), 0, &[]).schedule_background_refresh(
            observed_revision,
            observed_cache.clone(),
            |_| {
                launches += 1;
                Ok(())
            },
        );
        assert_eq!(observed_cache.models[0].id, "old-model");
        assert_eq!(
            launches, 0,
            "a failed peer must start cooldown before another launch"
        );
        assert_eq!(cache.models[0].id, "old-model");
        assert!(matches!(
            outcome,
            RefreshOutcome::Stale {
                refresh: BackgroundRefresh::Cooldown,
                last_failure: Some(reason),
            } if reason.contains("peer network failure")
        ));
    }

    #[test]
    #[serial]
    fn ensure_fresh_18_offline_env_blocks_forced_fetch() {
        let mars = tempdir().unwrap();
        let _offline = EnvVarGuard::set("MARS_OFFLINE", "1");

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api.json");
            then.status(200).json_body(sample_catalog_json());
        });
        let _api = EnvVarGuard::set("MARS_MODELS_API_URL", &server.url("/api.json"));

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Force);
        assert_model_cache_unavailable(result, "MARS_OFFLINE");
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    #[serial]
    fn ensure_fresh_19_concurrent_auto_refresh_hits_api_once() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let path = Arc::new(mars.path().to_path_buf());
        let path_a = Arc::clone(&path);
        let path_b = Arc::clone(&path);
        let fetch_hits = Arc::new(AtomicUsize::new(0));
        let (fetch_started_tx, fetch_started_rx) = mpsc::channel::<()>();
        let (release_fetch_tx, release_fetch_rx) = mpsc::channel::<()>();

        let fetch_hits_a = Arc::clone(&fetch_hits);
        let t1 = thread::spawn(move || {
            ensure_fresh_with_fetcher(&path_a, 24, RefreshMode::Synchronous, move || {
                fetch_hits_a.fetch_add(1, Ordering::SeqCst);
                fetch_started_tx.send(()).unwrap();
                release_fetch_rx.recv().unwrap();
                Ok(vec![sample_cached_model("fresh-model")])
            })
            .unwrap()
            .1
        });

        fetch_started_rx.recv().unwrap();

        let fetch_hits_b = Arc::clone(&fetch_hits);
        let t2 = thread::spawn(move || {
            ensure_fresh_with_fetcher(&path_b, 24, RefreshMode::Synchronous, move || {
                fetch_hits_b.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_cached_model("unexpected-second-refresh")])
            })
            .unwrap()
            .1
        });

        release_fetch_tx.send(()).unwrap();

        let outcome_a = t1.join().unwrap();
        let outcome_b = t2.join().unwrap();

        let outcomes = [outcome_a, outcome_b];
        let refreshed = outcomes
            .iter()
            .filter(|o| matches!(o, RefreshOutcome::Refreshed { .. }))
            .count();
        let already_fresh = outcomes
            .iter()
            .filter(|o| matches!(o, RefreshOutcome::AlreadyFresh))
            .count();

        assert_eq!(refreshed, 1);
        assert_eq!(already_fresh, 1);
        assert_eq!(fetch_hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_20_failed_fetch_cooldown_coalesces_sequential_calls() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let fetch_hits = Arc::new(AtomicUsize::new(0));

        let fetch_hits_a = Arc::clone(&fetch_hits);
        let (_cache_a, outcome_a) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                fetch_hits_a.fetch_add(1, Ordering::SeqCst);
                Err(MarsError::Http {
                    url: "https://example.test/api.json".to_string(),
                    status: 500,
                    message: "request failed with HTTP status 500".to_string(),
                })
            })
            .unwrap();

        let fetch_hits_b = Arc::clone(&fetch_hits);
        let (_cache_b, outcome_b) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                fetch_hits_b.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_cached_model("unexpected-second-refresh")])
            })
            .unwrap();

        assert!(matches!(
            outcome_a,
            RefreshOutcome::StaleFallback { reason } if reason.contains("fetch failed")
        ));
        assert_eq!(
            outcome_b,
            RefreshOutcome::StaleFallback {
                reason: FETCH_FAIL_COOLDOWN_REASON.to_string()
            }
        );
        assert_eq!(fetch_hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[serial]
    fn ensure_fresh_21_empty_catalog_cooldown_coalesces_sequential_calls() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let fetch_hits = Arc::new(AtomicUsize::new(0));

        let fetch_hits_a = Arc::clone(&fetch_hits);
        let (_cache_a, outcome_a) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                fetch_hits_a.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            })
            .unwrap();

        let fetch_hits_b = Arc::clone(&fetch_hits);
        let (_cache_b, outcome_b) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                fetch_hits_b.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_cached_model("unexpected-second-refresh")])
            })
            .unwrap();

        assert!(matches!(
            outcome_a,
            RefreshOutcome::StaleFallback { reason } if reason.contains("API returned empty catalog")
        ));
        assert_eq!(
            outcome_b,
            RefreshOutcome::StaleFallback {
                reason: FETCH_FAIL_COOLDOWN_REASON.to_string()
            }
        );
        assert_eq!(fetch_hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn zero_refresh_after_coalesces_workers_from_same_revision() {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("old-model")],
            &fresh_timestamp(),
        );
        let attempts = Arc::new(AtomicUsize::new(0));
        let first = {
            let attempts = Arc::clone(&attempts);
            ensure_fresh_with_fetcher_if_revision(
                mars.path(),
                0,
                RefreshMode::Synchronous,
                Some(0),
                move || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![sample_cached_model("new-model")])
                },
            )
            .unwrap()
        };
        let second = {
            let attempts = Arc::clone(&attempts);
            ensure_fresh_with_fetcher_if_revision(
                mars.path(),
                0,
                RefreshMode::Synchronous,
                Some(0),
                move || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![sample_cached_model("unexpected-second-fetch")])
                },
            )
            .unwrap()
        };
        assert!(matches!(first.1, RefreshOutcome::Refreshed { .. }));
        assert_eq!(second.1, RefreshOutcome::AlreadyFresh);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(read_snapshot(mars.path()).unwrap().revision, 1);
    }
}
