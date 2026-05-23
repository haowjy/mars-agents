use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::harness::registry::{self, HarnessId};
use crate::models::probes::cursor_cache::{self, CachedCursorProbeOutcome};
use crate::models::probes::opencode_cache::{self, CachedProbeOutcome};
use crate::models::probes::pi_cache::{self, CachedPiProbeOutcome};

#[derive(Debug, Clone)]
pub struct CapabilityCollectionOptions {
    pub offline: bool,
    pub allow_probe_refresh: bool,
}

impl Default for CapabilityCollectionOptions {
    fn default() -> Self {
        Self {
            offline: false,
            allow_probe_refresh: true,
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
    let mut executable = BTreeMap::new();
    let mut auth = BTreeMap::new();

    for descriptor in registry::descriptors() {
        let state = resolver.resolve(descriptor.binary);
        executable.insert(descriptor.id, state.clone());
        auth.insert(
            descriptor.id,
            native_auth_state(descriptor.id, &state, resolver, auth_probe_timeout()),
        );
    }

    let installed = executable
        .iter()
        .filter(|(_, state)| matches!(state, ExecutableState::Found { .. }))
        .map(|(id, _)| id)
        .map(|id| id.as_str().to_string())
        .collect::<HashSet<_>>();

    let opencode_offline = options.offline || !options.allow_probe_refresh;
    let pi_offline = options.offline || !options.allow_probe_refresh;
    let cursor_offline = options.offline || !options.allow_probe_refresh;

    CapabilitySnapshot {
        executable,
        auth,
        opencode: opencode_cache::probe_cached(&installed, opencode_offline),
        pi: pi_cache::probe_cached(&installed, pi_offline),
        cursor: cursor_cache::probe_cached(&installed, cursor_offline),
        offline: options.offline,
    }
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
            allow_probe_refresh: false,
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
}
