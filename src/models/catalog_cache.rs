//! Last-known-good models.dev snapshot and stale-while-revalidate lifecycle.
//!
//! One snapshot per user, in the Mars cache root, shared by every project and every Mars
//! build on the machine. It holds every provider; a project's `catalog_providers` filters
//! what it reads. The file name carries [`SNAPSHOT_SCHEMA`], so a build that reads another
//! format finds no file instead of rewriting the other build's.

use super::catalog_api::{CATALOG_HTTP_DEADLINE_SECS, fetch_models, retain_providers};
use crate::error::MarsError;
use mars_schema::models::{CachedModel, provider_key_from_display};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
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

/// Bump when an older build could not read the file. The file name is the version, so the old
/// and new builds each keep their own.
const SNAPSHOT_SCHEMA: u32 = 2;
/// What builds before the shared snapshot wrote into each project's `.mars/`. Never written
/// now; read only to seed a machine that has no snapshot yet.
const LEGACY_PROJECT_CACHE_FILE: &str = "models-cache.json";
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

fn snapshot_file_name() -> String {
    format!("models-catalog-v{SNAPSHOT_SCHEMA}.json")
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

fn read_fetch_fail_marker(dir: &Path) -> Option<FetchFailure> {
    let marker = dir.join(FETCH_FAIL_MARKER_FILE);
    let raw = std::fs::read_to_string(marker).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_fetch_fail_marker(dir: &Path, reason: &str) {
    let marker = dir.join(FETCH_FAIL_MARKER_FILE);
    let failure = FetchFailure {
        at: now_unix_secs_value(),
        reason: reason.to_string(),
    };
    let content = serde_json::to_vec(&failure).expect("fetch failure is serializable");
    if let Err(err) = crate::fs::atomic_write(&marker, &content) {
        tracing::debug!("failed to write models fetch failure marker: {err}");
    }
}

fn clear_fetch_fail_marker(dir: &Path) {
    let marker = dir.join(FETCH_FAIL_MARKER_FILE);
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

fn read_refresh_claim(dir: &Path) -> Result<Option<RefreshClaim>, MarsError> {
    let path = dir.join(REFRESH_CLAIM_FILE);
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

struct CatalogCache {
    /// The shared cache directory: snapshot, lock, claim and failure marker.
    dir: PathBuf,
    /// The project's `.mars/`, read only to seed a machine with no snapshot.
    seed_dir: Option<PathBuf>,
    ttl_hours: u32,
}

fn refresh_claim_is_owned(dir: &Path, token: &str) -> Result<bool, MarsError> {
    let _lock = crate::fs::FileLock::acquire(&dir.join(REFRESH_CLAIM_LOCK_FILE))?;
    Ok(read_refresh_claim(dir)?.is_some_and(|claim| claim.token == token))
}

fn release_refresh_claim(dir: &Path, token: &str) -> Result<(), MarsError> {
    let _lock = crate::fs::FileLock::acquire(&dir.join(REFRESH_CLAIM_LOCK_FILE))?;
    if read_refresh_claim(dir)?.is_some_and(|claim| claim.token == token) {
        std::fs::remove_file(dir.join(REFRESH_CLAIM_FILE))?;
    }
    Ok(())
}

struct RefreshClaimGuard<'a> {
    dir: &'a Path,
    token: &'a str,
}

impl Drop for RefreshClaimGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = release_refresh_claim(self.dir, self.token) {
            tracing::debug!("failed to clear catalog refresh claim: {error}");
        }
    }
}

/// The snapshot, refreshed per `mode`, filtered to the project's `providers`.
/// `project_mars_dir` is the project's `.mars/`: its pre-snapshot catalog seeds a machine that
/// has none.
pub fn ensure_fresh_with_catalog_providers(
    project_mars_dir: &Path,
    ttl_hours: u32,
    mode: RefreshMode,
    providers: &[String],
) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
    let (cache, outcome) = CatalogCache::new(Some(project_mars_dir), ttl_hours)?.ensure(mode)?;
    Ok((filtered(cache, providers), outcome))
}

/// Entry point for the hidden worker command. Never selects Background, so it cannot recurse.
pub fn run_background_refresh(
    ttl_hours: u32,
    expected_revision: u64,
    claim_token: &str,
) -> Result<(), MarsError> {
    CatalogCache::new(None, ttl_hours)?.run_worker(expected_revision, claim_token)
}

/// The snapshot as it is on disk, filtered to `providers`, with no refresh.
pub fn read_cache(project_mars_dir: &Path, providers: &[String]) -> Result<ModelsCache, MarsError> {
    let cache = CatalogCache::new(Some(project_mars_dir), 0)?.load()?.cache;
    Ok(filtered(cache, providers))
}

fn filtered(mut cache: ModelsCache, providers: &[String]) -> ModelsCache {
    retain_providers(&mut cache.models, providers);
    cache
}

impl CatalogCache {
    fn new(seed_dir: Option<&Path>, ttl_hours: u32) -> Result<Self, MarsError> {
        Ok(Self {
            dir: crate::platform::cache::global_cache_root()?,
            seed_dir: seed_dir.map(Path::to_path_buf),
            ttl_hours,
        })
    }

    fn snapshot_path(&self) -> PathBuf {
        self.dir.join(snapshot_file_name())
    }

    /// The shared snapshot; a missing or unreadable one falls back to the seed, then to empty.
    fn load(&self) -> Result<Snapshot, MarsError> {
        match read_snapshot(&self.snapshot_path()) {
            Ok(Some(snapshot)) => Ok(snapshot),
            Ok(None) => Ok(self.seed()),
            Err(err) => {
                tracing::debug!("models cache read failed, treating as empty: {err}");
                Ok(self.seed())
            }
        }
    }

    fn load_or_empty(&self) -> Snapshot {
        self.load().unwrap_or_else(|_| Snapshot::empty())
    }

    /// The project's pre-snapshot catalog, stale by construction so it is refreshed but serves
    /// an offline first run.
    fn seed(&self) -> Snapshot {
        let Some(seed_dir) = &self.seed_dir else {
            return Snapshot::empty();
        };
        let Ok(Some(mut snapshot)) = read_snapshot(&seed_dir.join(LEGACY_PROJECT_CACHE_FILE))
        else {
            return Snapshot::empty();
        };
        snapshot.cache.fetched_at = None;
        snapshot.revision = 0;
        snapshot
    }

    fn ensure(&self, mode: RefreshMode) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
        let dir = self.dir.as_path();
        let ttl_hours = self.ttl_hours;
        if mode == RefreshMode::Background && !is_mars_offline() {
            std::fs::create_dir_all(dir)?;
            let snapshot = self.load_or_empty();
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
        self.refresh_with_fetcher(mode, None, fetch_models)
    }

    fn claim_refresh(&self, observed_revision: u64) -> Result<RefreshClaimDecision, MarsError> {
        let dir = self.dir.as_path();
        let ttl_hours = self.ttl_hours;
        let _lock = crate::fs::FileLock::acquire(&dir.join(REFRESH_CLAIM_LOCK_FILE))?;
        // The worker holds its claim until snapshot and failure marker
        // updates are complete. Rechecking here closes the read-to-claim race.
        let snapshot = self.load_or_empty();
        let current_revision = snapshot.revision;
        let cache = snapshot.cache;
        let failure = read_fetch_fail_marker(dir);
        let last_failure = failure.as_ref().map(|failure| failure.reason.clone());
        let now = now_unix_secs_value();
        if read_refresh_claim(dir)?
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
            .tempfile_in(dir)?;
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
            &dir.join(REFRESH_CLAIM_FILE),
            &serde_json::to_vec(&claim).expect("refresh claim is serializable"),
        )?;
        Ok(RefreshClaimDecision::Claimed {
            token,
            cache,
            last_failure,
        })
    }

    fn schedule_background_refresh(
        &self,
        observed_revision: u64,
        prior: ModelsCache,
        spawn: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> (ModelsCache, RefreshOutcome) {
        let dir = self.dir.as_path();
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
                        if let Err(release_error) = release_refresh_claim(dir, &token) {
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
                    last_failure: read_fetch_fail_marker(dir).map(|failure| failure.reason),
                },
            ),
        }
    }

    fn spawn_background_refresh(
        &self,
        expected_revision: u64,
        claim_token: &str,
    ) -> std::io::Result<()> {
        let ttl_hours = self.ttl_hours;
        let program = std::env::current_exe()?;
        let mut args = vec![
            OsString::from("models"),
            OsString::from("__refresh-catalog"),
            OsString::from("--refresh-after-hours"),
            OsString::from(ttl_hours.to_string()),
        ];
        args.extend([
            OsString::from("--expected-revision"),
            OsString::from(expected_revision.to_string()),
            OsString::from("--claim-token"),
            OsString::from(claim_token),
        ]);
        crate::platform::process::spawn_detached(program.as_os_str(), &args)
    }

    fn run_worker(&self, expected_revision: u64, claim_token: &str) -> Result<(), MarsError> {
        let dir = self.dir.as_path();
        let _claim = RefreshClaimGuard {
            dir,
            token: claim_token,
        };
        if !refresh_claim_is_owned(dir, claim_token)? {
            return Ok(());
        }
        self.refresh_with_fetcher(RefreshMode::Synchronous, Some(expected_revision), || {
            fetch_models()
        })?;
        Ok(())
    }

    fn refresh_with_fetcher<F>(
        &self,
        mode: RefreshMode,
        expected_revision: Option<u64>,
        fetcher: F,
    ) -> Result<(ModelsCache, RefreshOutcome), MarsError>
    where
        F: FnOnce() -> Result<Vec<CachedModel>, MarsError>,
    {
        let dir = self.dir.as_path();
        let ttl_hours = self.ttl_hours;
        std::fs::create_dir_all(dir)?;

        let effective_mode = if is_mars_offline() {
            RefreshMode::Offline
        } else {
            mode
        };

        let prior = self.load_or_empty().cache;

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

        let lock_path = dir.join(".models-cache.lock");
        let _guard = crate::fs::FileLock::acquire(&lock_path)?;

        let snapshot = self.load_or_empty();
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
            if let Some(last_fail) = read_fetch_fail_marker(dir)
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
                    dir,
                    &Snapshot {
                        cache: cache.clone(),
                        revision,
                    },
                )?;
                clear_fetch_fail_marker(dir);
                Ok((cache, RefreshOutcome::Refreshed { models_count }))
            }
            Ok(_) => fallback_to_stale_or_error(
                dir,
                under_lock,
                "API returned empty catalog".to_string(),
                "API returned an empty catalog and no prior cache exists".to_string(),
            ),
            Err(err) => fallback_to_stale_or_error(
                dir,
                under_lock,
                format!("fetch failed: {err}"),
                format!("automatic refresh failed: {err}"),
            ),
        }
    }
}

fn fallback_to_stale_or_error(
    dir: &Path,
    under_lock: ModelsCache,
    stale_reason: String,
    unavailable_reason: String,
) -> Result<(ModelsCache, RefreshOutcome), MarsError> {
    if is_usable(&under_lock) {
        write_fetch_fail_marker(dir, &stale_reason);
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

/// `None` when there is no file at `path`.
fn read_snapshot(path: &Path) -> Result<Option<Snapshot>, MarsError> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_json::from_str::<Snapshot>(&content)
            .map(|mut snapshot| {
                // Rows from before the provider key existed.
                for model in &mut snapshot.cache.models {
                    if model.provider_key.is_empty() {
                        model.provider_key = provider_key_from_display(&model.provider);
                    }
                }
                Some(snapshot)
            })
            .map_err(|e| {
                crate::error::ConfigError::Invalid {
                    message: format!("failed to parse models cache: {e}"),
                }
                .into()
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MarsError::Io {
            operation: "read models cache".to_string(),
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_snapshot(dir: &Path, snapshot: &Snapshot) -> Result<(), MarsError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(snapshot_file_name());
    let content = serde_json::to_vec(snapshot).map_err(|e| crate::error::ConfigError::Invalid {
        message: format!("failed to serialize models cache: {e}"),
    })?;
    crate::fs::atomic_write(&path, &content)?;
    Ok(())
}
