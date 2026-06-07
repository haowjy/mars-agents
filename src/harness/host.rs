use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::harness::registry::{self, HarnessId};
use crate::models::probes::ProbeRefreshMode;
use crate::models::probes::cursor_cache::CachedCursorProbeOutcome;
use crate::models::probes::opencode_cache::CachedProbeOutcome;
use crate::models::probes::pi_cache::CachedPiProbeOutcome;
use crate::models::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};

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

#[derive(Debug, Clone)]
pub struct CapabilitySnapshot {
    pub executable: BTreeMap<HarnessId, ExecutableState>,
    pub auth: BTreeMap<HarnessId, AuthState>,
    pub opencode: CachedProbeOutcome,
    pub pi: CachedPiProbeOutcome,
    pub cursor: CachedCursorProbeOutcome,
    pub offline: bool,
}

impl CapabilitySnapshot {
    pub fn installed_harnesses(&self) -> HashSet<String> {
        self.executable
            .iter()
            .filter(|(_, state)| matches!(state, ExecutableState::Found { .. }))
            .map(|(id, _)| id)
            .map(|id| id.as_str().to_string())
            .collect()
    }
}

/// Command-scoped lazy capability session.
///
/// Executable/auth checks are collected immediately. Harness probe checks are
/// loaded lazily on first use per harness and memoized for the command.
#[derive(Debug, Clone)]
pub struct CapabilitySession {
    executable: BTreeMap<HarnessId, ExecutableState>,
    auth: BTreeMap<HarnessId, AuthState>,
    installed: HashSet<String>,
    offline: bool,
    probe_refresh: ProbeRefreshMode,
    opencode: Option<CachedProbeOutcome>,
    pi: Option<CachedPiProbeOutcome>,
    cursor: Option<CachedCursorProbeOutcome>,
}

impl CapabilitySession {
    pub fn collect(options: &CapabilityCollectionOptions) -> Self {
        Self::collect_with_resolver(options, &PathExecutableResolver)
    }

    pub(crate) fn collect_without_auth(options: &CapabilityCollectionOptions) -> Self {
        Self::collect_with_resolver_without_auth(options, &PathExecutableResolver)
    }

    pub fn collect_with_resolver(
        options: &CapabilityCollectionOptions,
        resolver: &dyn ExecutableResolver,
    ) -> Self {
        Self::collect_with_resolver_inner(options, resolver, true)
    }

    pub(crate) fn collect_with_resolver_without_auth(
        options: &CapabilityCollectionOptions,
        resolver: &dyn ExecutableResolver,
    ) -> Self {
        Self::collect_with_resolver_inner(options, resolver, false)
    }

    fn collect_with_resolver_inner(
        options: &CapabilityCollectionOptions,
        resolver: &dyn ExecutableResolver,
        collect_auth: bool,
    ) -> Self {
        let mut executable = BTreeMap::new();
        let mut auth = BTreeMap::new();

        for descriptor in registry::descriptors() {
            let state = resolver.resolve(descriptor.binary);
            executable.insert(descriptor.id, state.clone());
            let auth_state = if collect_auth {
                native_auth_state(descriptor.id, &state, resolver, auth_probe_timeout())
            } else {
                AuthState::Unknown {
                    reason: "auth not collected".to_string(),
                }
            };
            auth.insert(descriptor.id, auth_state);
        }

        let installed = executable
            .iter()
            .filter(|(_, state)| matches!(state, ExecutableState::Found { .. }))
            .map(|(id, _)| id.as_str().to_string())
            .collect::<HashSet<_>>();

        Self {
            executable,
            auth,
            installed,
            offline: options.offline,
            probe_refresh: options.probe_refresh,
            opencode: None,
            pi: None,
            cursor: None,
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

    pub fn offline(&self) -> bool {
        self.offline
    }

    pub fn executable_snapshot(&self) -> BTreeMap<HarnessId, ExecutableState> {
        self.executable.clone()
    }

    pub fn auth_snapshot(&self) -> BTreeMap<HarnessId, AuthState> {
        self.auth.clone()
    }

    pub fn opencode_outcome(&mut self) -> &CachedProbeOutcome {
        self.opencode.get_or_insert_with(|| {
            cached_opencode_outcome(&self.installed, self.offline, self.probe_refresh)
        })
    }

    pub fn loaded_opencode_outcome(&self) -> Option<&CachedProbeOutcome> {
        self.opencode.as_ref()
    }

    pub fn loaded_pi_outcome(&self) -> Option<&CachedPiProbeOutcome> {
        self.pi.as_ref()
    }

    pub fn loaded_cursor_outcome(&self) -> Option<&CachedCursorProbeOutcome> {
        self.cursor.as_ref()
    }

    pub fn loaded_opencode_probe_result(&self) -> Option<&OpenCodeProbeResult> {
        self.loaded_opencode_outcome()
            .and_then(CachedProbeOutcome::result)
    }

    pub fn loaded_pi_probe_result(&self) -> Option<&PiProbeResult> {
        self.loaded_pi_outcome()
            .and_then(CachedPiProbeOutcome::result)
    }

    pub fn loaded_cursor_probe_result(&self) -> Option<&CursorProbeResult> {
        self.loaded_cursor_outcome()
            .and_then(CachedCursorProbeOutcome::result)
    }

    pub fn pi_outcome(&mut self) -> &CachedPiProbeOutcome {
        self.pi.get_or_insert_with(|| {
            cached_pi_outcome(&self.installed, self.offline, self.probe_refresh)
        })
    }

    pub fn cursor_outcome(&mut self) -> &CachedCursorProbeOutcome {
        self.cursor.get_or_insert_with(|| {
            cached_cursor_outcome(&self.installed, self.offline, self.probe_refresh)
        })
    }

    pub fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult> {
        self.opencode_outcome().result().cloned()
    }

    pub fn pi_probe_result(&mut self) -> Option<PiProbeResult> {
        self.pi_outcome().result().cloned()
    }

    pub fn cursor_probe_result(&mut self) -> Option<CursorProbeResult> {
        self.cursor_outcome().result().cloned()
    }

    pub fn into_snapshot(mut self) -> CapabilitySnapshot {
        let opencode = self.opencode.take().unwrap_or_else(|| {
            cached_opencode_outcome(&self.installed, self.offline, self.probe_refresh)
        });
        let pi = self.pi.take().unwrap_or_else(|| {
            cached_pi_outcome(&self.installed, self.offline, self.probe_refresh)
        });
        let cursor = self.cursor.take().unwrap_or_else(|| {
            cached_cursor_outcome(&self.installed, self.offline, self.probe_refresh)
        });

        CapabilitySnapshot {
            executable: self.executable,
            auth: self.auth,
            opencode,
            pi,
            cursor,
            offline: self.offline,
        }
    }
}

fn cached_opencode_outcome(
    installed: &HashSet<String>,
    is_offline: bool,
    probe_refresh: ProbeRefreshMode,
) -> CachedProbeOutcome {
    crate::models::probes::opencode_cache::probe_cached(installed, is_offline, probe_refresh)
}

fn cached_pi_outcome(
    installed: &HashSet<String>,
    is_offline: bool,
    probe_refresh: ProbeRefreshMode,
) -> CachedPiProbeOutcome {
    crate::models::probes::pi_cache::probe_cached(installed, is_offline, probe_refresh)
}

fn cached_cursor_outcome(
    installed: &HashSet<String>,
    is_offline: bool,
    probe_refresh: ProbeRefreshMode,
) -> CachedCursorProbeOutcome {
    crate::models::probes::cursor_cache::probe_cached(installed, is_offline, probe_refresh)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableState {
    Found { path: PathBuf },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    NotApplicable,
    Authenticated,
    Unauthenticated,
    Unknown { reason: String },
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

pub fn collect_capability_snapshot(options: &CapabilityCollectionOptions) -> CapabilitySnapshot {
    collect_capability_snapshot_with_resolver(options, &PathExecutableResolver)
}

pub fn collect_capability_snapshot_with_resolver(
    options: &CapabilityCollectionOptions,
    resolver: &dyn ExecutableResolver,
) -> CapabilitySnapshot {
    CapabilitySession::collect_with_resolver(options, resolver).into_snapshot()
}

pub fn native_harness_authenticated(harness: &str) -> bool {
    native_auth_state_for_name(harness) == AuthState::Authenticated
}

pub fn native_auth_state_for_name(harness: &str) -> AuthState {
    let Some(id) = registry::parse(harness) else {
        return AuthState::Unknown {
            reason: "unknown harness".to_string(),
        };
    };

    let resolver = PathExecutableResolver;
    let state = resolver.resolve(registry::descriptor(id).binary);
    native_auth_state(id, &state, &resolver, auth_probe_timeout())
}

fn native_auth_state(
    id: HarnessId,
    executable: &ExecutableState,
    resolver: &dyn ExecutableResolver,
    timeout: Duration,
) -> AuthState {
    let (binary, args) = match id {
        HarnessId::Codex => ("codex", &["login", "status"][..]),
        HarnessId::Claude => ("claude", &["auth", "status"][..]),
        _ => return AuthState::NotApplicable,
    };

    if !matches!(executable, ExecutableState::Found { .. }) {
        return AuthState::Unauthenticated;
    }

    run_status_command(binary, args, timeout, resolver)
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
    fn snapshot_marks_installed_harnesses_from_resolver() {
        let mut resolver = FakeResolver::default();
        resolver.map.insert(
            "pi".to_string(),
            ExecutableState::Found {
                path: PathBuf::from("/tmp/pi"),
            },
        );

        let options = CapabilityCollectionOptions {
            offline: true,
            probe_refresh: ProbeRefreshMode::Skip,
        };
        let snapshot = collect_capability_snapshot_with_resolver(&options, &resolver);

        let installed = snapshot.installed_harnesses();
        assert!(installed.contains("pi"));
        assert!(!installed.contains("codex"));
    }

    #[test]
    fn native_auth_for_non_native_harness_is_not_applicable() {
        let resolver = FakeResolver::default();
        let state = native_auth_state(
            HarnessId::Pi,
            &ExecutableState::Found {
                path: PathBuf::from("/tmp/pi"),
            },
            &resolver,
            Duration::from_secs(1),
        );

        assert_eq!(state, AuthState::NotApplicable);
    }

    #[test]
    fn resolve_binary_path_returns_none_when_missing() {
        let resolver = FakeResolver::default();
        assert_eq!(resolve_binary_path("codex", &resolver), None);
    }

    #[test]
    fn probe_refresh_skip_does_not_force_offline_mode() {
        let options = CapabilityCollectionOptions {
            offline: false,
            probe_refresh: ProbeRefreshMode::Skip,
        };
        let session = CapabilitySession::collect_with_resolver(&options, &FakeResolver::default());
        assert!(!session.offline());
    }
}
