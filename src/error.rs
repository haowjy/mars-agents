use crate::types::managed_cmd;
use std::path::{Path, PathBuf};

/// Config-level errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found: {path}")]
    NotFound { path: PathBuf },

    /// `refusal` says why no project can be started at `start`; then `mars init` is no way out.
    #[error("{}", project_root_not_found(start, refusal.as_deref()))]
    ProjectRootNotFound {
        start: PathBuf,
        refusal: Option<String>,
    },

    #[error("{message}")]
    Invalid { message: String },

    #[error(transparent)]
    Filter(#[from] mars_schema::filter::FilterError),

    #[error(transparent)]
    Entry(#[from] mars_schema::dependency::EntryError),

    #[error(transparent)]
    PluginSelector(#[from] mars_schema::dependency::PluginSelectorError),

    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

fn project_root_not_found(start: &Path, refusal: Option<&str>) -> String {
    let searched = format!(
        "no mars.toml found from {} to filesystem root",
        start.display()
    );
    match refusal {
        Some(refusal) => format!("{searched}, and {refusal}."),
        None => format!(
            "{searched}. Run `{init}` to start a project here, or `{user}` to use your user level.",
            init = managed_cmd("mars init"),
            user = managed_cmd("mars --user <command>"),
        ),
    }
}

/// Lock file errors
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("lock file corrupt: {message}")]
    Corrupt { message: String },

    /// The write journal refuses to recover. `mars repair` leaves the journal in place,
    /// so it is not a way out; the message says what is.
    #[error("{message}")]
    PendingWrite { message: String },

    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolution errors
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    #[error(
        "invalid {engine} version requirement `{requirement}` in package `{package}`: {message}"
    )]
    InvalidEngineRequirement {
        package: String,
        engine: String,
        requirement: String,
        message: String,
    },

    #[error("invalid running {engine} version `{version}`: {message}")]
    InvalidRunningEngineVersion {
        engine: String,
        version: String,
        message: String,
    },

    #[error("engine requirements are unsatisfiable for `{name}`: {message}")]
    RequiresMarsUnsatisfiable { name: String, message: String },

    #[error("source `{name}` is incompatible with the running engine: {message}")]
    RequiresEngineIncompatible { name: String, message: String },

    #[error("version conflict for `{name}`: {message}")]
    VersionConflict { name: String, message: String },

    #[error(
        "duplicate source identity: `{existing_name}` and `{duplicate_name}` both resolve to `{source_id}`"
    )]
    DuplicateSourceIdentity {
        existing_name: String,
        duplicate_name: String,
        source_id: String,
    },

    #[error(
        "source `{name}` was referenced with conflicting identities: existing `{existing}`, incoming `{incoming}`"
    )]
    SourceIdentityMismatch {
        name: String,
        existing: String,
        incoming: String,
    },
}

/// Top-level error type aggregating all module errors
#[derive(Debug, thiserror::Error)]
pub enum MarsError {
    #[error("{source}")]
    WithDiagnostics {
        diagnostics: Vec<crate::diagnostic::Diagnostic>,
        #[source]
        source: Box<MarsError>,
    },

    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    #[error("lock error: {0}")]
    Lock(#[from] LockError),

    #[error("source error: {source_name}: {message}")]
    Source {
        source_name: String,
        message: String,
    },

    #[error(
        "source error: {source_name}: subpath `{subpath}` escapes checkout root `{}`",
        checkout_root.display()
    )]
    SubpathTraversal {
        source_name: String,
        subpath: String,
        checkout_root: PathBuf,
    },

    #[error(
        "source error: {source_name}: subpath `{subpath}` does not exist under checkout root `{}`",
        checkout_root.display()
    )]
    SubpathMissing {
        source_name: String,
        subpath: String,
        checkout_root: PathBuf,
    },

    #[error(
        "source error: {source_name}: subpath `{subpath}` is not a directory under checkout root `{}`",
        checkout_root.display()
    )]
    SubpathNotDirectory {
        source_name: String,
        subpath: String,
        checkout_root: PathBuf,
    },

    #[error(
        "discovery collision in `{source_name}`: {kind} `{item_name}` found at `{}` and `{}`",
        path_a.display(),
        path_b.display()
    )]
    DiscoveryCollision {
        source_name: String,
        kind: String,
        item_name: String,
        path_a: PathBuf,
        path_b: PathBuf,
    },

    #[error(
        "source error: {source_name}: plugin manifest path `{manifest_path}` escapes package root `{}`",
        package_root.display()
    )]
    ManifestDeclaredPathEscape {
        source_name: String,
        manifest_path: String,
        package_root: PathBuf,
    },

    #[error(
        "source error: {source_name}: plugin manifest path `{manifest_path}` does not exist under package root `{}`",
        package_root.display()
    )]
    ManifestDeclaredPathMissing {
        source_name: String,
        manifest_path: String,
        package_root: PathBuf,
    },

    #[error("resolution failed: {0}")]
    Resolution(#[from] ResolutionError),

    #[error("{item} is provided by both `{source_a}` and `{source_b}`")]
    Collision {
        item: String,
        source_a: String,
        source_b: String,
    },

    #[error("invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("frozen violation: {message}")]
    FrozenViolation { message: String },

    #[error("{message}{report}")]
    Selection {
        code: &'static str,
        message: String,
        report: Box<crate::routing::report::RouteDecisionReport>,
    },

    #[error("commit {commit} is not reachable in {url}")]
    CommitUnreachable { commit: String, url: String },

    /// Internal control-flow signal: the resolver detected that an already-resolved
    /// package would select a different ref under the full accumulated constraint set.
    /// Caught by the `resolve()` driver to trigger a fresh-context restart.
    /// Never surfaces to end users.
    #[error("(internal: resolution restart needed for `{package}`)")]
    ResolutionRestartNeeded { package: String },

    /// Link operation error — conflict, missing target, or invalid link metadata.
    #[error("link error: {target}: {message}")]
    Link { target: String, message: String },

    #[error(
        "models cache is unavailable: {reason}. Run `{cmd}` to populate it.",
        cmd = managed_cmd("mars models refresh"),
    )]
    ModelCacheUnavailable { reason: String },

    /// A name matched no content layer; `code` (`agent_not_found`, `agent_renamed`,
    /// `skill_not_found`, `skill_renamed`) is the JSON error code a caller tells a missing item
    /// from other failures by.
    #[error("{message}")]
    NotFound { code: &'static str, message: String },

    #[error("{operation} failed for {}: {source}", path.display())]
    Io {
        operation: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("HTTP error: {url} — {status}: {message}")]
    Http {
        url: String,
        status: u16,
        message: String,
    },

    #[error("git command failed: `{command}` — {message}")]
    GitCli { command: String, message: String },

    #[error("release aborted: {message}")]
    Release { message: String },

    #[error("{release}; rollback failed: {rollback}")]
    ReleaseRollback {
        #[source]
        release: Box<MarsError>,
        rollback: String,
    },

    #[error("internal error: {0}")]
    Internal(String),
}

impl MarsError {
    /// Map error variants to CLI exit codes.
    ///
    /// - 2: resolution/validation/config error
    /// - 3: source, I/O, HTTP, or git CLI error
    pub fn exit_code(&self) -> i32 {
        match self {
            MarsError::WithDiagnostics { source, .. } => source.exit_code(),
            MarsError::Link { .. }
            | MarsError::Config(_)
            | MarsError::Lock(_)
            | MarsError::Resolution(_)
            | MarsError::Collision { .. }
            | MarsError::InvalidRequest { .. }
            | MarsError::FrozenViolation { .. }
            | MarsError::Selection { .. }
            | MarsError::CommitUnreachable { .. } => 2,
            MarsError::Source { .. }
            | MarsError::SubpathTraversal { .. }
            | MarsError::SubpathMissing { .. }
            | MarsError::SubpathNotDirectory { .. }
            | MarsError::DiscoveryCollision { .. }
            | MarsError::ManifestDeclaredPathEscape { .. }
            | MarsError::ManifestDeclaredPathMissing { .. }
            | MarsError::ModelCacheUnavailable { .. }
            | MarsError::NotFound { .. }
            | MarsError::Io { .. }
            | MarsError::Http { .. }
            | MarsError::GitCli { .. }
            | MarsError::Release { .. }
            | MarsError::ReleaseRollback { .. }
            | MarsError::Internal(_) => 3,
            MarsError::ResolutionRestartNeeded { .. } => {
                unreachable!("ResolutionRestartNeeded is an internal signal caught by resolve()")
            }
        }
    }

    pub fn diagnostics(&self) -> &[crate::diagnostic::Diagnostic] {
        match self {
            Self::WithDiagnostics { diagnostics, .. } => diagnostics,
            _ => &[],
        }
    }

    pub fn root_cause(&self) -> &Self {
        match self {
            Self::WithDiagnostics { source, .. } => source.root_cause(),
            _ => self,
        }
    }
}

impl From<std::io::Error> for MarsError {
    fn from(source: std::io::Error) -> Self {
        MarsError::Io {
            operation: "I/O operation".to_string(),
            path: PathBuf::from("<unknown>"),
            source,
        }
    }
}

impl MarsError {
    /// An I/O error whose message names the path the operation was working on.
    pub(crate) fn io(operation: &str, path: &Path, source: std::io::Error) -> Self {
        MarsError::Io {
            operation: operation.to_string(),
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mars_error_exit_codes_match_spec() {
        let cases = vec![
            (
                MarsError::Config(ConfigError::Invalid {
                    message: "bad config".to_string(),
                }),
                2,
            ),
            (
                MarsError::Lock(LockError::Corrupt {
                    message: "bad lock".to_string(),
                }),
                2,
            ),
            (
                MarsError::Collision {
                    item: "coder".to_string(),
                    source_a: "base".to_string(),
                    source_b: "custom".to_string(),
                },
                2,
            ),
            (
                MarsError::InvalidRequest {
                    message: "bad flag combination".to_string(),
                },
                2,
            ),
            (
                MarsError::FrozenViolation {
                    message: "lock file would change but --frozen is set".to_string(),
                },
                2,
            ),
            (
                MarsError::CommitUnreachable {
                    commit: "abc123".to_string(),
                    url: "https://example.com/repo.git".to_string(),
                },
                2,
            ),
            (
                MarsError::Link {
                    target: ".claude".to_string(),
                    message: "conflicts found".to_string(),
                },
                2,
            ),
            (
                MarsError::Source {
                    source_name: "origin".to_string(),
                    message: "network failed".to_string(),
                },
                3,
            ),
            (
                MarsError::SubpathTraversal {
                    source_name: "origin".to_string(),
                    subpath: "../escape".to_string(),
                    checkout_root: PathBuf::from("/tmp/root"),
                },
                3,
            ),
            (
                MarsError::SubpathMissing {
                    source_name: "origin".to_string(),
                    subpath: "plugins/foo".to_string(),
                    checkout_root: PathBuf::from("/tmp/root"),
                },
                3,
            ),
            (
                MarsError::SubpathNotDirectory {
                    source_name: "origin".to_string(),
                    subpath: "plugins/foo".to_string(),
                    checkout_root: PathBuf::from("/tmp/root"),
                },
                3,
            ),
            (
                MarsError::DiscoveryCollision {
                    source_name: "origin".to_string(),
                    kind: "skill".to_string(),
                    item_name: "plan".to_string(),
                    path_a: PathBuf::from("skills/a"),
                    path_b: PathBuf::from("skills/b"),
                },
                3,
            ),
            (
                MarsError::ManifestDeclaredPathEscape {
                    source_name: "origin".to_string(),
                    manifest_path: "./../escape".to_string(),
                    package_root: PathBuf::from("/tmp/root"),
                },
                3,
            ),
            (
                MarsError::ManifestDeclaredPathMissing {
                    source_name: "origin".to_string(),
                    manifest_path: "./missing".to_string(),
                    package_root: PathBuf::from("/tmp/root"),
                },
                3,
            ),
            (
                MarsError::ModelCacheUnavailable {
                    reason: "MARS_OFFLINE is set and no cached catalog is available".to_string(),
                },
                3,
            ),
            (
                MarsError::Io {
                    operation: "read file".to_string(),
                    path: PathBuf::from("/tmp/file"),
                    source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
                },
                3,
            ),
            (
                MarsError::Http {
                    url: "https://example.com/archive.tar.gz".to_string(),
                    status: 503,
                    message: "service unavailable".to_string(),
                },
                3,
            ),
            (
                MarsError::GitCli {
                    command: "git ls-remote --tags https://example.com/repo".to_string(),
                    message: "fatal: repository not found".to_string(),
                },
                3,
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(
                err.exit_code(),
                expected,
                "unexpected exit code for error: {err}"
            );
        }
    }
}
