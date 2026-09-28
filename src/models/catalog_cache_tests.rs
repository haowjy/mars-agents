use super::*;
use crate::models::resolve_models_refresh_control;
use httpmock::prelude::*;
use serial_test::serial;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use tempfile::tempdir;

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

fn write_cache(mars_dir: &Path, cache: &ModelsCache) -> Result<(), MarsError> {
    write_snapshot(
        mars_dir,
        &Snapshot {
            cache: cache.clone(),
            revision: 0,
        },
    )
}

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
        let (_, outcome) =
            ensure_fresh_with_fetcher_if_revision(mars.path(), 0, RefreshMode::Force, None, || {
                if fetch {
                    Ok(Vec::new())
                } else {
                    Err(MarsError::ModelCacheUnavailable {
                        reason: "network down".into(),
                    })
                }
            })
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
