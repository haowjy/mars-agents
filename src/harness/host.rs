use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::harness::registry;
use crate::harness::registry::{HarnessClass, SlugShape};
use crate::models::probes::ProbeRefreshMode;
use crate::models::probes::cache::CachedProbeOutcome;
use crate::models::probes::{Listing, ObservedOutcome, ProbeObservation};
use mars_schema::HarnessId;

#[derive(Debug, Clone)]
pub struct CapabilityCollectionOptions {
    /// `MARS_OFFLINE` — skip network/catalog assumptions; probes treat env as offline.
    pub offline: bool,
    pub probe_refresh: ProbeRefreshMode,
}

impl Default for CapabilityCollectionOptions {
    fn default() -> Self {
        Self {
            offline: false,
            probe_refresh: ProbeRefreshMode::Background,
        }
    }
}

/// Listing success and refresh freshness are independent: a failed latest
/// attempt may retain last-good support without implying current auth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListingEvidence {
    pub succeeded: bool,
    pub latest_attempt_ok: bool,
}

/// Command-scoped lazy capability session.
///
/// Executable checks are collected immediately. Each probe-backed harness's listing is
/// loaded lazily on first use and memoized for the command.
#[derive(Debug, Clone)]
pub struct CapabilitySession {
    installed: HashSet<String>,
    offline: bool,
    probe_refresh: ProbeRefreshMode,
    outcomes: BTreeMap<HarnessId, ObservedOutcome<CachedProbeOutcome<Listing>>>,
}

impl CapabilitySession {
    pub fn collect(options: &CapabilityCollectionOptions) -> Self {
        Self::collect_with_resolver(options, &PathExecutableResolver)
    }

    pub fn collect_with_resolver(
        options: &CapabilityCollectionOptions,
        resolver: &dyn ExecutableResolver,
    ) -> Self {
        let installed = mars_schema::harness::ALL
            .iter()
            .filter(|id| matches!(resolver.resolve(id.as_str()), ExecutableState::Found { .. }))
            .map(|id| id.as_str().to_string())
            .collect();

        Self {
            installed,
            offline: options.offline,
            probe_refresh: options.probe_refresh,
            outcomes: BTreeMap::new(),
        }
    }

    pub fn installed_harnesses(&self) -> HashSet<String> {
        self.installed.clone()
    }

    pub(crate) fn extend_installed_harnesses<I>(&mut self, harnesses: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.installed.extend(harnesses);
    }

    fn observed(&mut self, harness: HarnessId) -> &ObservedOutcome<CachedProbeOutcome<Listing>> {
        self.outcomes
            .entry(harness)
            .or_insert_with(|| match registry::probe_spec(harness) {
                Some(spec) => spec
                    .probe
                    .load(&self.installed, self.offline, self.probe_refresh),
                None => ObservedOutcome {
                    outcome: CachedProbeOutcome::Unavailable,
                    observation: None,
                },
            })
    }

    /// The harness's listing outcome, probing on first use. A native harness has none.
    pub fn outcome(&mut self, harness: HarnessId) -> &CachedProbeOutcome<Listing> {
        &self.observed(harness).outcome
    }

    /// The outcome already loaded for routing or listing; never probes.
    pub fn loaded_outcome(&self, harness: HarnessId) -> Option<&CachedProbeOutcome<Listing>> {
        self.outcomes
            .get(&harness)
            .map(|observed| &observed.outcome)
    }

    pub fn listing(&mut self, harness: HarnessId) -> Option<Listing> {
        self.outcome(harness).result().cloned()
    }

    pub fn probe_observation(&mut self, harness: HarnessId) -> Option<ProbeObservation> {
        self.observed(harness).observation.clone()
    }

    pub fn listing_evidence(&mut self, harness: HarnessId) -> ListingEvidence {
        let outcome = self.outcome(harness);
        ListingEvidence {
            succeeded: outcome.result().is_some_and(Listing::is_evidence),
            latest_attempt_ok: outcome.latest_attempt_ok(),
        }
    }

    /// Providers Mars has observed, for `provider_order` warnings; `None` while pending.
    /// Reads only outcomes already loaded for routing or listing: a diagnostic must never
    /// trigger a probe. A harness whose outcome isn't loaded counts as never probed.
    pub fn known_providers(
        &mut self,
        provider_order: &[String],
        catalog_slugs: &[String],
    ) -> Option<HashSet<String>> {
        use crate::harness::known_providers::{ProbeListing, known_providers};
        if provider_order.is_empty() {
            return None;
        }
        let mut probes = Vec::new();
        for descriptor in registry::descriptors() {
            let Some(spec) = registry::probe_spec(descriptor.id) else {
                continue;
            };
            if spec.slugs != SlugShape::Qualified
                || !self.installed.contains(descriptor.id.as_str())
            {
                continue;
            }
            probes.push(match self.loaded_outcome(descriptor.id) {
                None | Some(CachedProbeOutcome::Unavailable) => ProbeListing::NeverProbed,
                Some(CachedProbeOutcome::Failed(_)) => ProbeListing::Failed,
                Some(outcome) => ProbeListing::Listed(outcome.result()?.slugs.clone()),
            });
        }
        known_providers(catalog_slugs, &probes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableState {
    Found { path: PathBuf },
    Missing,
}

/// Runtime authentication evidence for one harness route.
///
/// `Authenticated` and `ImpliedByListing` are equal-strength evidence: credentials are
/// configured, not proven valid or funded. They differ in freshness: native status runs
/// per command, while listing evidence is as old as the probe cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// No auth observation was made: an ungated listing (OpenCode), a gated harness
    /// without a successful listing, or native materialization's deliberate skip.
    Unchecked,
    /// A native status command succeeded.
    Authenticated,
    /// A credential-gated listing succeeded on its latest attempt.
    ImpliedByListing,
    /// A credential-gated listing's latest attempt failed (possibly logged out); support
    /// still comes from the last good listing.
    ListingFailed,
    /// A native status command reported no usable login.
    Unauthenticated,
    /// A native status command was inconclusive (timeout, spawn failure).
    Unknown { reason: String },
}

impl AuthState {
    /// Whether this observation counts as runtime auth evidence for eligibility.
    pub fn is_runtime_evidence(&self) -> bool {
        matches!(self, Self::Authenticated | Self::ImpliedByListing)
    }
}

pub trait ExecutableResolver {
    fn resolve(&self, binary: &str) -> ExecutableState;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PathExecutableResolver;

impl ExecutableResolver for PathExecutableResolver {
    fn resolve(&self, binary: &str) -> ExecutableState {
        if let Ok(path) = which::which(binary) {
            return ExecutableState::Found { path };
        }

        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                if let Ok(path) = which::which(format!("{binary}.{ext}")) {
                    return ExecutableState::Found { path };
                }
            }
        }

        ExecutableState::Missing
    }
}

/// Native auth is observed lazily once per command, independently of model candidates.
/// Unknown results are cached too; neither a backup nor another alias retries them.
#[derive(Debug, Default)]
pub struct NativeAuthCache {
    states: std::cell::RefCell<BTreeMap<HarnessId, AuthState>>,
}

impl NativeAuthCache {
    pub fn state(&self, harness: &str) -> AuthState {
        let Some(id) = mars_schema::harness::parse(harness) else {
            return AuthState::Unknown {
                reason: "unknown harness".to_string(),
            };
        };
        self.states
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| native_auth_state_for_name(id.as_str()))
            .clone()
    }
}

pub fn native_auth_state_for_name(harness: &str) -> AuthState {
    let Some(id) = mars_schema::harness::parse(harness) else {
        return AuthState::Unknown {
            reason: "unknown harness".to_string(),
        };
    };

    let resolver = PathExecutableResolver;
    let state = resolver.resolve(id.as_str());
    native_auth_state(id, &state, &resolver, auth_probe_timeout())
}

fn native_auth_state(
    id: HarnessId,
    executable: &ExecutableState,
    resolver: &dyn ExecutableResolver,
    timeout: Duration,
) -> AuthState {
    let HarnessClass::Native { auth_status, .. } = registry::descriptor(id).class else {
        return AuthState::Unchecked;
    };

    if !matches!(executable, ExecutableState::Found { .. }) {
        return AuthState::Unauthenticated;
    }

    run_status_command(id.as_str(), auth_status, timeout, resolver)
}

pub fn auth_probe_timeout() -> Duration {
    std::env::var("MARS_NATIVE_HARNESS_AUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(2))
}

fn run_status_command(
    command: &str,
    args: &[&str],
    timeout: Duration,
    resolver: &dyn ExecutableResolver,
) -> AuthState {
    let program = resolve_binary_path(command, resolver).unwrap_or_else(|| PathBuf::from(command));

    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return AuthState::Unknown {
                reason: format!("spawn failed: {error}"),
            };
        }
    };

    match child.wait_timeout(timeout) {
        Ok(Some(status)) if status.success() => AuthState::Authenticated,
        Ok(Some(_)) => AuthState::Unauthenticated,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            AuthState::Unknown {
                reason: "auth probe timeout".to_string(),
            }
        }
        Err(error) => AuthState::Unknown {
            reason: format!("auth probe wait failed: {error}"),
        },
    }
}

pub fn resolve_binary_path(binary: &str, resolver: &dyn ExecutableResolver) -> Option<PathBuf> {
    match resolver.resolve(binary) {
        ExecutableState::Found { path } => Some(path),
        ExecutableState::Missing => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeResolver {
        map: HashMap<String, ExecutableState>,
    }

    impl ExecutableResolver for FakeResolver {
        fn resolve(&self, binary: &str) -> ExecutableState {
            self.map
                .get(binary)
                .cloned()
                .unwrap_or(ExecutableState::Missing)
        }
    }

    #[test]
    fn session_marks_installed_harnesses_from_resolver() {
        let mut resolver = FakeResolver::default();
        resolver.map.insert(
            "pi".to_string(),
            ExecutableState::Found {
                path: PathBuf::from("/tmp/pi"),
            },
        );

        let session = CapabilitySession::collect_with_resolver(
            &CapabilityCollectionOptions {
                offline: true,
                probe_refresh: ProbeRefreshMode::Skip,
            },
            &resolver,
        );

        let installed = session.installed_harnesses();
        assert!(installed.contains("pi"));
        assert!(!installed.contains("codex"));
    }

    #[test]
    fn native_auth_for_non_native_harness_is_unchecked() {
        let resolver = FakeResolver::default();
        let state = native_auth_state(
            HarnessId::Pi,
            &ExecutableState::Found {
                path: PathBuf::from("/tmp/pi"),
            },
            &resolver,
            Duration::from_secs(1),
        );

        assert_eq!(state, AuthState::Unchecked);
    }

    #[test]
    fn resolve_binary_path_returns_none_when_missing() {
        let resolver = FakeResolver::default();
        assert_eq!(resolve_binary_path("codex", &resolver), None);
    }
}
