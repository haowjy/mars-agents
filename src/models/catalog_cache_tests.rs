use super::*;
use crate::models::resolve_models_refresh_control;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

fn cache_with(models: Vec<CachedModel>, fetched_at: Option<&str>) -> ModelsCache {
    ModelsCache {
        models,
        fetched_at: fetched_at.map(str::to_string),
    }
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

#[test]
fn is_fresh_rejects_zero_ttl_empty_unparseable_and_future_timestamps() {
    let model = || sample_cached_model("m");
    let now = now_unix_secs_value();
    for (cache, ttl_hours, expected) in [
        (cache_with(vec![model()], Some(&now.to_string())), 0, false),
        (cache_with(Vec::new(), Some(&now.to_string())), 24, false),
        (cache_with(vec![model()], None), 24, false),
        (
            cache_with(vec![model()], Some("not-a-timestamp")),
            24,
            false,
        ),
        // A clock-skewed future timestamp makes the age meaningless.
        (
            cache_with(vec![model()], Some(&(now + 3600).to_string())),
            24,
            false,
        ),
        (
            cache_with(vec![model()], Some(&(now - 48 * 3600).to_string())),
            24,
            false,
        ),
        (
            cache_with(vec![model()], Some(&(now - 60).to_string())),
            24,
            true,
        ),
    ] {
        assert_eq!(
            is_fresh(&cache, ttl_hours),
            expected,
            "{cache:?} ttl={ttl_hours}"
        );
    }
}

#[test]
fn missing_cache_fetch_failure_errors_instead_of_reporting_an_empty_catalog() {
    let mars = tempdir().unwrap();

    let result = ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, || {
        Err(MarsError::Http {
            url: "https://example.test/api.json".to_string(),
            status: 500,
            message: "server error".to_string(),
        })
    });
    assert_model_cache_unavailable(result, "automatic refresh failed");
    assert!(!mars.path().join(CACHE_FILE).exists());
}

#[test]
fn empty_or_corrupt_cache_without_prior_snapshot_refetches() {
    for corrupt in [false, true] {
        let mars = tempdir().unwrap();
        if corrupt {
            write_raw_cache_file(mars.path(), "{ not-json ");
        } else {
            write_cache_state(mars.path(), Vec::new(), &fresh_timestamp());
        }

        let (cache, outcome) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, || {
                Ok(vec![sample_cached_model("fetched")])
            })
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Refreshed { .. }));
        assert_eq!(cache.models[0].id, "fetched");
    }
}

#[test]
fn empty_or_corrupt_cache_offline_errors() {
    for corrupt in [false, true] {
        let mars = tempdir().unwrap();
        if corrupt {
            write_raw_cache_file(mars.path(), "{ not-json ");
        } else {
            write_cache_state(mars.path(), Vec::new(), &fresh_timestamp());
        }

        let result = ensure_fresh(mars.path(), 24, RefreshMode::Offline);
        assert_model_cache_unavailable(result, "--no-refresh-models was passed");
    }
}

#[test]
fn resolve_models_refresh_control_maps_flags_to_modes() {
    use crate::models::probes::ProbeRefreshMode;
    for (refresh, no_refresh, expected_catalog, expected_probe) in [
        (
            false,
            false,
            RefreshMode::Background,
            ProbeRefreshMode::Background,
        ),
        (false, true, RefreshMode::Offline, ProbeRefreshMode::Skip),
        (
            true,
            false,
            RefreshMode::Force,
            ProbeRefreshMode::Synchronous,
        ),
    ] {
        let control = resolve_models_refresh_control(refresh, no_refresh).unwrap();
        assert_eq!(control.catalog_mode, expected_catalog);
        assert_eq!(control.probe_refresh, expected_probe);
    }
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
fn cooldown_after_failed_or_empty_fetch_coalesces_sequential_calls() {
    for empty_catalog in [false, true] {
        let mars = tempdir().unwrap();
        write_cache_state(
            mars.path(),
            vec![sample_cached_model("stale-model")],
            &stale_timestamp(),
        );

        let fetch_hits = Arc::new(AtomicUsize::new(0));

        let hits = Arc::clone(&fetch_hits);
        let (_cache_a, outcome_a) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                hits.fetch_add(1, Ordering::SeqCst);
                if empty_catalog {
                    Ok(Vec::new())
                } else {
                    Err(MarsError::Http {
                        url: "https://example.test/api.json".to_string(),
                        status: 500,
                        message: "request failed with HTTP status 500".to_string(),
                    })
                }
            })
            .unwrap();

        let hits = Arc::clone(&fetch_hits);
        let (_cache_b, outcome_b) =
            ensure_fresh_with_fetcher(mars.path(), 24, RefreshMode::Synchronous, move || {
                hits.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_cached_model("unexpected-second-refresh")])
            })
            .unwrap();

        let expected_failure = if empty_catalog {
            "API returned empty catalog"
        } else {
            "fetch failed"
        };
        assert!(matches!(
            outcome_a,
            RefreshOutcome::StaleFallback { reason } if reason.contains(expected_failure)
        ));
        assert_eq!(
            outcome_b,
            RefreshOutcome::StaleFallback {
                reason: FETCH_FAIL_COOLDOWN_REASON.to_string()
            }
        );
        assert_eq!(fetch_hits.load(Ordering::SeqCst), 1);
    }
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
