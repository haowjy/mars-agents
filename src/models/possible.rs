//! Derived harness × model evidence. No Possible file or alias can create rows.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::config::targets::HarnessScope;
use crate::harness::host::{CapabilitySession, ListingEvidence};
use crate::harness::registry::{self, HarnessId, ListingAuth};
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::ProbeObservation;
use crate::routing::slug;

use super::ModelsCache;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PossibleRow {
    pub harness: HarnessId,
    pub harness_model_id: String,
    pub provider: Option<String>,
    pub model_id: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    Pi,
    Cursor,
    #[serde(rename = "opencode")]
    OpenCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    Enumerated {
        probe: ProbeKind,
        observed_at: u64,
        auth_gated: bool,
        latest_attempt_ok: bool,
        last_error: Option<String>,
    },
    Inferred {
        catalog_fetched_at: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlistedReason {
    OutOfScope,
    NotInstalled,
    NoCatalog,
    ListingUnavailable,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum HarnessPossible {
    Listed(Vec<PossibleRow>),
    Unlisted(UnlistedReason),
}

/// One diagnostic per harness, derived from the same session outcome as its
/// Possible rows. A retained failed refresh is distinct from no listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingIssue {
    Unavailable {
        harness: HarnessId,
        last_error: Option<String>,
    },
    RetainedAfterFailedRefresh {
        harness: HarnessId,
        last_error: Option<String>,
    },
}

pub trait PossibleSource {
    fn rows_for(&mut self, harness: HarnessId) -> &HarnessPossible;
}

/// Lazily asks the command-scoped capability session only for requested harnesses.
/// Display calls `all_rows`; routing callers may ask for a single harness.
pub struct SessionPossibleSource<'a> {
    catalog: &'a ModelsCache,
    session: &'a mut CapabilitySession,
    scope: &'a HarnessScope,
    installed: HashSet<HarnessId>,
    memo: BTreeMap<HarnessId, HarnessPossible>,
}

impl<'a> SessionPossibleSource<'a> {
    pub fn new(
        catalog: &'a ModelsCache,
        session: &'a mut CapabilitySession,
        scope: &'a HarnessScope,
    ) -> Self {
        let installed = session
            .installed_harnesses()
            .iter()
            .filter_map(|name| registry::parse(name))
            .collect();
        Self {
            catalog,
            session,
            scope,
            installed,
            memo: BTreeMap::new(),
        }
    }

    pub fn all_rows(&mut self) -> Vec<PossibleRow> {
        registry::all()
            .iter()
            .flat_map(|harness| match self.rows_for(*harness) {
                HarnessPossible::Listed(rows) => rows.clone(),
                HarnessPossible::Unlisted(_) => Vec::new(),
            })
            .collect()
    }

    /// Report listing trouble for installed, in-scope probe-backed harnesses
    /// already projected into the memo. Display calls `all_rows` first; this
    /// method never reads a probe cache again.
    pub fn listing_issues(&mut self) -> Vec<ListingIssue> {
        self.memo
            .iter()
            .filter_map(|(harness, state)| {
                let retained = match state {
                    HarnessPossible::Unlisted(UnlistedReason::ListingUnavailable) => false,
                    HarnessPossible::Listed(_) if harness.native_provider().is_none() => {
                        if self.session.listing_evidence(*harness).latest_attempt_ok {
                            return None;
                        }
                        true
                    }
                    _ => return None,
                };
                let last_error = self
                    .session
                    .probe_observation(*harness)
                    .and_then(|observation| observation.last_error);
                Some(if retained {
                    ListingIssue::RetainedAfterFailedRefresh {
                        harness: *harness,
                        last_error,
                    }
                } else {
                    ListingIssue::Unavailable {
                        harness: *harness,
                        last_error,
                    }
                })
            })
            .collect()
    }

    fn project(&mut self, harness: HarnessId) -> HarnessPossible {
        if !self.scope.permits(harness.as_str()) {
            return HarnessPossible::Unlisted(UnlistedReason::OutOfScope);
        }
        if !self.installed.contains(&harness) {
            return HarnessPossible::Unlisted(UnlistedReason::NotInstalled);
        }
        if let Some(provider) = harness.native_provider() {
            let mut rows = self
                .catalog
                .models
                .iter()
                .filter(|model| slug::providers_match(&model.provider, provider))
                .map(|model| {
                    let launch = resolve_harness_model(
                        harness,
                        &model.id,
                        None,
                        Some(&model.id),
                        None,
                        Some(&model.provider),
                    );
                    PossibleRow {
                        harness,
                        harness_model_id: launch.harness_model_id,
                        provider: Some(slug::normalize_provider(&model.provider)),
                        model_id: slug::parse(&model.id)
                            .map_or(model.id.as_str(), |parts| parts.model_id)
                            .to_string(),
                        provenance: Provenance::Inferred {
                            catalog_fetched_at: self.catalog.fetched_at.clone(),
                        },
                    }
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| a.harness_model_id.cmp(&b.harness_model_id));
            rows.dedup_by(|a, b| a.harness_model_id == b.harness_model_id);
            return if rows.is_empty() {
                HarnessPossible::Unlisted(UnlistedReason::NoCatalog)
            } else {
                HarnessPossible::Listed(rows)
            };
        }

        // Each outcome is memoized by CapabilitySession. The typed seam owns the
        // listing-success and latest-attempt policy for every probe-backed harness.
        let (slugs, compatible, enumeration_succeeded) = match harness {
            HarnessId::Pi => {
                let result = self.session.pi_outcome().result();
                (
                    result.map(|probe| probe.model_slugs.iter().cloned().collect::<Vec<_>>()),
                    result.map(|probe| probe.compatible),
                    result.is_some_and(|probe| probe.model_probe_success),
                )
            }
            HarnessId::OpenCode => {
                let result = self.session.opencode_outcome().result();
                (
                    result.map(|probe| probe.model_slugs.clone()),
                    Some(true),
                    result.is_some_and(|probe| probe.model_probe_success),
                )
            }
            HarnessId::Cursor => {
                let result = self.session.cursor_outcome().result();
                (
                    result.map(|probe| probe.slugs.clone()),
                    Some(true),
                    result.is_some_and(|probe| probe.model_probe_success),
                )
            }
            _ => unreachable!("native harness handled above"),
        };
        if compatible == Some(false) {
            return HarnessPossible::Unlisted(UnlistedReason::Incompatible);
        }
        let listing = self.session.listing_evidence(harness);
        if !enumeration_succeeded {
            return HarnessPossible::Unlisted(UnlistedReason::ListingUnavailable);
        }
        let observation = self.session.probe_observation(harness).unwrap_or_default();
        project_enumerated(harness, slugs.unwrap_or_default(), listing, observation)
    }
}

impl PossibleSource for SessionPossibleSource<'_> {
    fn rows_for(&mut self, harness: HarnessId) -> &HarnessPossible {
        if !self.memo.contains_key(&harness) {
            let projected = self.project(harness);
            self.memo.insert(harness, projected);
        }
        self.memo.get(&harness).expect("memoized harness")
    }
}

fn project_enumerated(
    harness: HarnessId,
    slugs: Vec<String>,
    listing: ListingEvidence,
    observation: ProbeObservation,
) -> HarnessPossible {
    let (probe, auth_gated) = match harness.class() {
        crate::harness::registry::HarnessClass::ProbeBacked { listing } => {
            let probe = match harness {
                HarnessId::Pi => ProbeKind::Pi,
                HarnessId::OpenCode => ProbeKind::OpenCode,
                HarnessId::Cursor => ProbeKind::Cursor,
                _ => unreachable!("native harness has no listing"),
            };
            (probe, listing == ListingAuth::Gated)
        }
        _ => unreachable!("native harness handled above"),
    };
    // A lock-less, command-local probe may succeed without a cache write. In
    // that case there is no retained timestamp; it was observed in this call.
    let observed_at = observation.observed_at.unwrap_or_else(now_unix_secs);
    let mut rows = slugs
        .into_iter()
        .filter_map(|raw| {
            let (provider, model_id) = if harness == HarnessId::Cursor {
                (None, raw.clone())
            } else {
                let parts = slug::parse(&raw)?;
                (Some(parts.provider.to_string()), parts.model_id.to_string())
            };
            let launch = resolve_harness_model(
                harness,
                &raw,
                Some(&raw),
                Some(&model_id),
                None,
                provider.as_deref(),
            );
            Some(PossibleRow {
                harness,
                harness_model_id: launch.harness_model_id,
                provider,
                model_id,
                provenance: Provenance::Enumerated {
                    probe,
                    observed_at,
                    auth_gated,
                    latest_attempt_ok: listing.latest_attempt_ok,
                    last_error: if listing.latest_attempt_ok {
                        None
                    } else {
                        observation.last_error.clone()
                    },
                },
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.harness_model_id.cmp(&b.harness_model_id));
    rows.dedup_by(|a, b| a.harness_model_id == b.harness_model_id);
    HarnessPossible::Listed(rows)
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::host::{CapabilityCollectionOptions, ExecutableResolver, ExecutableState};
    use crate::models::probes::ProbeRefreshMode;
    use crate::models::probes::cursor_cache::CachedCursorProbeOutcome;
    use crate::models::probes::opencode_cache::CachedProbeOutcome;
    use crate::models::probes::pi_cache::CachedPiProbeOutcome;
    use crate::models::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};

    struct Installed(Vec<&'static str>);
    impl ExecutableResolver for Installed {
        fn resolve(&self, binary: &str) -> ExecutableState {
            if self.0.contains(&binary) {
                ExecutableState::Found {
                    path: format!("/fake/{binary}").into(),
                }
            } else {
                ExecutableState::Missing
            }
        }
    }

    fn catalog() -> ModelsCache {
        serde_json::from_value(serde_json::json!({
            "fetched_at": "123",
            "models": [
                {"id": "claude-opus-4-6", "provider": "anthropic"},
                {"id": "gpt-5", "provider": "openai"},
                {"id": "gpt-5.5", "provider": "openai-codex"},
                {"id": "gemini-2.5", "provider": "google"}
            ]
        }))
        .unwrap()
    }

    fn session(installed: Vec<&'static str>) -> CapabilitySession {
        CapabilitySession::collect_with_resolver(
            &CapabilityCollectionOptions {
                offline: true,
                probe_refresh: ProbeRefreshMode::Skip,
            },
            &Installed(installed),
        )
    }

    #[test]
    fn native_rows_are_inferred_only_for_matching_providers_and_installed_scope() {
        let mut session = session(vec!["claude", "codex", "pi"]);
        let scope = HarnessScope::Only([HarnessId::Claude, HarnessId::Codex].into_iter().collect());
        let catalog = catalog();
        let mut source = SessionPossibleSource::new(&catalog, &mut session, &scope);
        let rows = source.all_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].harness, HarnessId::Claude);
        assert_eq!(rows[0].harness_model_id, "claude-opus-4-6");
        assert!(
            rows.iter()
                .all(|row| matches!(row.provenance, Provenance::Inferred { .. }))
        );
        assert!(
            rows.iter()
                .any(|row| row.provider.as_deref() == Some("openai"))
        );
        assert_eq!(
            source.rows_for(HarnessId::Pi),
            &HarnessPossible::Unlisted(UnlistedReason::OutOfScope)
        );
    }

    #[test]
    fn native_provider_display_case_and_variants_normalize_before_dedup() {
        let catalog: ModelsCache = serde_json::from_value(serde_json::json!({
            "models": [
                {"id": "claude-opus-4-6", "provider": "Anthropic"},
                {"id": "claude-opus-4-6", "provider": "ANTHROPIC-CLAUDE"},
                {"id": "gpt-6-sol", "provider": "OpenAI-Codex"},
                {"id": "gpt-6-sol", "provider": "OpenAI"}
            ]
        }))
        .unwrap();
        let mut session = session(vec!["claude", "codex"]);
        let mut source =
            SessionPossibleSource::new(&catalog, &mut session, &HarnessScope::Unrestricted);
        let rows = source.all_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider.as_deref(), Some("anthropic"));
        assert_eq!(rows[1].provider.as_deref(), Some("openai"));
    }

    #[test]
    fn missing_or_uninstalled_harness_and_alias_only_input_create_no_rows() {
        let empty: ModelsCache = serde_json::from_value(serde_json::json!({"models": []})).unwrap();
        let mut session = session(vec!["claude"]);
        let mut source =
            SessionPossibleSource::new(&empty, &mut session, &HarnessScope::Unrestricted);
        assert_eq!(
            source.rows_for(HarnessId::Claude),
            &HarnessPossible::Unlisted(UnlistedReason::NoCatalog)
        );
        assert_eq!(
            source.rows_for(HarnessId::Pi),
            &HarnessPossible::Unlisted(UnlistedReason::NotInstalled)
        );
        assert!(source.all_rows().is_empty());
        // No alias argument enters this API: even an authored alias for a model
        // not present in the catalog/listings cannot create a Possible row.
    }

    #[test]
    fn installed_pi_without_listing_is_unavailable_not_incompatible() {
        let mut session = session(vec!["pi"]);
        let catalog = catalog();
        let mut source =
            SessionPossibleSource::new(&catalog, &mut session, &HarnessScope::Unrestricted);
        assert_eq!(
            source.rows_for(HarnessId::Pi),
            &HarnessPossible::Unlisted(UnlistedReason::ListingUnavailable)
        );
    }

    #[test]
    fn successful_empty_cursor_listing_is_listed_without_auth_implication() {
        let mut session = session(vec!["cursor"]);
        session.set_cursor_outcome_for_test(CachedCursorProbeOutcome::Hit(CursorProbeResult {
            model_probe_success: true,
            slugs: Vec::new(),
            error: None,
        }));
        assert!(!session.listing_evidence(HarnessId::Cursor).succeeded);
        let catalog = catalog();
        let mut source =
            SessionPossibleSource::new(&catalog, &mut session, &HarnessScope::Unrestricted);
        assert_eq!(
            source.rows_for(HarnessId::Cursor),
            &HarnessPossible::Listed(Vec::new())
        );
    }

    #[test]
    fn probe_rows_keep_exact_slug_provider_and_fresh_or_failed_provenance() {
        for (harness, slugs, expected_provider, expected_model, gated) in [
            (
                HarnessId::Pi,
                vec!["openai-codex/gpt-5".into()],
                Some("openai-codex"),
                "gpt-5",
                true,
            ),
            (
                HarnessId::OpenCode,
                vec!["opencode-go/glm-5.2".into()],
                Some("opencode-go"),
                "glm-5.2",
                false,
            ),
            (
                HarnessId::Cursor,
                vec!["composer-2.5".into()],
                None,
                "composer-2.5",
                true,
            ),
        ] {
            for latest_attempt_ok in [true, false] {
                let HarnessPossible::Listed(rows) = project_enumerated(
                    harness,
                    slugs.clone(),
                    ListingEvidence {
                        succeeded: true,
                        latest_attempt_ok,
                    },
                    ProbeObservation {
                        observed_at: Some(123),
                        last_error: Some("refresh failed".into()),
                    },
                ) else {
                    panic!("expected rows")
                };
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].harness_model_id, slugs[0]);
                assert_eq!(rows[0].provider.as_deref(), expected_provider);
                assert_eq!(rows[0].model_id, expected_model);
                assert!(matches!(&rows[0].provenance, Provenance::Enumerated {
                    observed_at: 123,
                    auth_gated,
                    latest_attempt_ok: status,
                    last_error,
                    ..
                } if *auth_gated == gated && *status == latest_attempt_ok &&
                    last_error.as_deref() == if latest_attempt_ok { None } else { Some("refresh failed") }));
            }
        }
    }

    #[test]
    fn retained_session_outcomes_are_enumerated_and_scope_does_not_probe_others() {
        let mut session = session(vec!["pi", "cursor", "opencode"]);
        session.set_pi_outcome_for_test(CachedPiProbeOutcome::StaleFailed(PiProbeResult {
            compatible: true,
            model_probe_success: true,
            model_slugs: HashSet::from(["openai/gpt-5".into()]),
            ..PiProbeResult::default()
        }));
        session.set_cursor_outcome_for_test(CachedCursorProbeOutcome::Hit(CursorProbeResult {
            model_probe_success: true,
            slugs: vec!["composer-2.5".into()],
            error: None,
        }));
        session.set_opencode_outcome_for_test(CachedProbeOutcome::Hit(OpenCodeProbeResult {
            model_probe_success: true,
            model_slugs: vec!["opencode-go/glm-5.2".into()],
            error: None,
        }));
        let scope = HarnessScope::Only([HarnessId::Pi].into_iter().collect());
        {
            let catalog = catalog();
            let mut source = SessionPossibleSource::new(&catalog, &mut session, &scope);
            let HarnessPossible::Listed(rows) = source.rows_for(HarnessId::Pi) else {
                panic!("expected Pi")
            };
            assert_eq!(rows[0].harness_model_id, "openai/gpt-5");
            assert!(matches!(
                rows[0].provenance,
                Provenance::Enumerated {
                    latest_attempt_ok: false,
                    ..
                }
            ));
            assert_eq!(
                source.rows_for(HarnessId::Cursor),
                &HarnessPossible::Unlisted(UnlistedReason::OutOfScope)
            );
        }
        assert!(session.loaded_cursor_outcome().is_some()); // preloaded fixture, not accessed by source
    }

    #[test]
    fn per_harness_projection_keeps_other_probe_outcomes_unloaded() {
        let mut session = session(vec!["pi", "cursor", "opencode"]);
        let catalog = catalog();
        {
            let mut source =
                SessionPossibleSource::new(&catalog, &mut session, &HarnessScope::Unrestricted);
            assert!(matches!(
                source.rows_for(HarnessId::Pi),
                HarnessPossible::Unlisted(UnlistedReason::ListingUnavailable)
            ));
        }
        assert!(session.loaded_pi_outcome().is_some());
        assert!(session.loaded_cursor_outcome().is_none());
        assert!(session.loaded_opencode_outcome().is_none());
    }

    #[test]
    #[serial_test::serial]
    fn existing_cache_entries_supply_observed_time_and_retained_error() {
        struct CacheDirGuard(Option<std::ffi::OsString>);
        impl Drop for CacheDirGuard {
            fn drop(&mut self) {
                if let Some(value) = self.0.take() {
                    unsafe { std::env::set_var("MARS_CACHE_DIR", value) };
                } else {
                    unsafe { std::env::remove_var("MARS_CACHE_DIR") };
                }
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("availability");
        std::fs::create_dir(&cache).unwrap();
        let _guard = CacheDirGuard(std::env::var_os("MARS_CACHE_DIR"));
        unsafe { std::env::set_var("MARS_CACHE_DIR", temp.path()) };
        let pi = PiProbeResult {
            compatible: true,
            model_probe_success: true,
            model_slugs: HashSet::from(["openai/gpt-5".into()]),
            ..PiProbeResult::default()
        };
        std::fs::write(
            cache.join("pi.json"),
            serde_json::json!({
                "schema_version": 3, "harness": "pi", "fetched_at": 100,
                "last_attempt_at": 101, "last_error": "listing timed out", "result": pi
            })
            .to_string(),
        )
        .unwrap();
        let open = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5".into()],
            model_probe_success: true,
            error: None,
        };
        std::fs::write(
            cache.join("opencode-probe.json"),
            serde_json::json!({
                "schema_version": 1, "fetched_at": 102,
                "last_attempt_at": 102, "last_error": null, "result": open
            })
            .to_string(),
        )
        .unwrap();
        let cursor = CursorProbeResult {
            slugs: vec!["composer-2.5".into()],
            model_probe_success: true,
            error: None,
        };
        std::fs::write(
            cache.join("cursor-probe.json"),
            serde_json::json!({
                "schema_version": 1, "fetched_at": 103,
                "last_attempt_at": 103, "last_error": null, "result": cursor
            })
            .to_string(),
        )
        .unwrap();
        let mut session = CapabilitySession::collect_with_resolver(
            &CapabilityCollectionOptions {
                offline: false,
                probe_refresh: ProbeRefreshMode::Skip,
            },
            &Installed(vec!["pi", "opencode", "cursor"]),
        );
        // The session owns one atomic observation. A later cache mutation must
        // not rewrite the provenance of results already loaded into it.
        session.pi_outcome();
        session.opencode_outcome();
        session.cursor_outcome();
        std::fs::remove_file(cache.join("pi.json")).unwrap();
        std::fs::remove_file(cache.join("opencode-probe.json")).unwrap();
        std::fs::remove_file(cache.join("cursor-probe.json")).unwrap();
        let catalog = catalog();
        let mut source =
            SessionPossibleSource::new(&catalog, &mut session, &HarnessScope::Unrestricted);
        let rows = source.all_rows();
        assert_eq!(rows.len(), 3);
        for row in rows {
            let Provenance::Enumerated {
                observed_at,
                latest_attempt_ok,
                last_error,
                ..
            } = row.provenance
            else {
                panic!("expected enumerated")
            };
            let (expected_at, expected_ok, expected_error) = match row.harness {
                HarnessId::Pi => (100, false, Some("listing timed out")),
                HarnessId::OpenCode => (102, true, None),
                HarnessId::Cursor => (103, true, None),
                _ => unreachable!(),
            };
            assert_eq!(observed_at, expected_at);
            assert_eq!(latest_attempt_ok, expected_ok);
            assert_eq!(last_error.as_deref(), expected_error);
        }
    }
}
