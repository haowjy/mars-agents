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
