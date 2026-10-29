//! Derived harness × model evidence. No Possible file or alias can create rows.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::config::targets::HarnessScope;
use crate::harness::host::{CapabilitySession, ListingEvidence};
use crate::harness::registry::{self, ListingAuth};
use crate::models::harness_model::resolve_harness_model;
use crate::models::probes::ProbeObservation;
use crate::routing::slug;
use mars_schema::HarnessId;

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
            .filter_map(|name| mars_schema::harness::parse(name))
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
        mars_schema::harness::ALL
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
                    HarnessPossible::Listed(_) if registry::native_provider(*harness).is_none() => {
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
        if let Some(provider) = registry::native_provider(harness) {
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
    let (probe, auth_gated) = match registry::descriptor(harness).class {
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
    use crate::models::probes::CursorProbeResult;
    use crate::models::probes::ProbeRefreshMode;
    use crate::models::probes::cursor_cache::CachedCursorProbeOutcome;

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
    fn unlisted_reasons_distinguish_no_catalog_missing_harness_and_missing_listing() {
        // No alias argument enters this API: even an authored alias for a model
        // not present in the catalog/listing cannot create a Possible row.
        let empty: ModelsCache = serde_json::from_value(serde_json::json!({"models": []})).unwrap();
        let mut session = session(vec!["claude", "pi"]);
        let mut source =
            SessionPossibleSource::new(&empty, &mut session, &HarnessScope::Unrestricted);
        assert_eq!(
            source.rows_for(HarnessId::Claude),
            &HarnessPossible::Unlisted(UnlistedReason::NoCatalog)
        );
        // Installed but unprobed: unavailable, which is not the same as
        // incompatible and must not be reported as an empty listing.
        assert_eq!(
            source.rows_for(HarnessId::Pi),
            &HarnessPossible::Unlisted(UnlistedReason::ListingUnavailable)
        );
        assert_eq!(
            source.rows_for(HarnessId::Codex),
            &HarnessPossible::Unlisted(UnlistedReason::NotInstalled)
        );
        assert!(source.all_rows().is_empty());
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
}
