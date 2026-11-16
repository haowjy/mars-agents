use semver::Version;

use super::engine::{EngineFailure, describe};
use super::{Need, Rooted};
use crate::diagnostic::SchemaDiagnostic;

/// Why resolution stopped. Each kind carries what its message needs, and [`Failure::message`]
/// is the one formatter: `mars sync` prints that text, and so does Flow.
#[derive(Debug, Clone)]
pub enum Failure {
    /// No version meets every constraint, or the constraints cannot hold on one ref.
    Conflict {
        name: String,
        requesters: Vec<Requester>,
        detail: String,
    },
    /// Two names for one source, or one name for two sources.
    Identity(Identity),
    EngineIncompatible {
        name: String,
        failures: Vec<EngineFailure>,
        message: String,
    },
    /// Every candidate version was excluded by its engine requirement.
    EngineUnsatisfiable {
        name: String,
        excluded: Vec<(Version, Vec<EngineFailure>)>,
    },
    EngineRequirementInvalid {
        name: String,
        engine: String,
        requirement: String,
        message: String,
    },
    EngineVersionInvalid {
        engine: String,
        version: String,
        message: String,
    },
    Plugin {
        name: String,
        detail: PluginDetail,
        plugin: Option<String>,
        text: String,
    },
    Subpath {
        name: String,
        subpath: String,
        detail: SubpathDetail,
    },
    /// A package's manifest is not TOML.
    Unreadable {
        name: String,
        rooted: Rooted,
        diagnostics: Vec<SchemaDiagnostic>,
    },
    /// A package, or the root, declares a path dependency and the answerer refused it.
    Path {
        name: String,
        declared_in: String,
    },
    /// A `failed` answer, or a `missing` one that came from no lock.
    Unavailable {
        /// The package being resolved when the answer failed.
        package: String,
        need: Need,
        reason: String,
    },
    Bounds {
        limit: BoundsLimit,
        at: String,
    },
    /// Restarts keep changing one name's version.
    Oscillation {
        name: String,
        steps: Vec<Step>,
    },
    /// `--frozen` and the lock disagree.
    Frozen {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requester {
    pub by: String,
    pub constraint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    OneSourceTwoNames {
        existing: String,
        duplicate: String,
        source: String,
    },
    OneNameTwoSources {
        name: String,
        existing: String,
        incoming: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDetail {
    NotAMarketplace,
    NotListed,
    Refused,
    ChoiceNeeded,
    NotOnePackage,
    UnusablePath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpathDetail {
    Missing,
    NotADirectory,
    Traversal,
}

impl SubpathDetail {
    /// What is wrong with the folder, as the end of a sentence about it.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Missing => "does not exist in the source",
            Self::NotADirectory => "is not a directory in the source",
            Self::Traversal => "escapes the source",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundsLimit {
    Packages,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub version: Option<String>,
    pub commit: Option<String>,
}

impl Failure {
    /// The text `mars sync` prints for this failure.
    pub fn message(&self) -> String {
        match self {
            Self::Conflict { name, detail, .. } => {
                format!("version conflict for `{name}`: {detail}")
            }
            Self::Identity(Identity::OneSourceTwoNames {
                existing,
                duplicate,
                source,
            }) => format!(
                "duplicate source identity: `{existing}` and `{duplicate}` both resolve to `{source}`"
            ),
            Self::Identity(Identity::OneNameTwoSources {
                name,
                existing,
                incoming,
            }) => format!(
                "source `{name}` was referenced with conflicting identities: existing `{existing}`, incoming `{incoming}`"
            ),
            Self::EngineIncompatible { name, message, .. } => {
                format!("source `{name}` is incompatible with the running engine: {message}")
            }
            Self::EngineUnsatisfiable { name, excluded } => format!(
                "engine requirements are unsatisfiable for `{name}`: {}",
                unsatisfiable_detail(excluded)
            ),
            Self::EngineRequirementInvalid {
                name,
                engine,
                requirement,
                message,
            } => format!(
                "invalid {engine} version requirement `{requirement}` in package `{name}`: {message}"
            ),
            Self::EngineVersionInvalid {
                engine,
                version,
                message,
            } => format!("invalid running {engine} version `{version}`: {message}"),
            Self::Plugin { text, .. } => format!("invalid request: {text}"),
            Self::Subpath {
                name,
                subpath,
                detail,
            } => format!(
                "source error: {name}: subpath `{subpath}` {}",
                detail.reason()
            ),
            Self::Unreadable {
                name, diagnostics, ..
            } => format!(
                "failed to read the manifest of `{name}`: {}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::Path { name, declared_in } => format!(
                "`{declared_in}` declares a path dependency `{name}`, which this resolution cannot read: a path names a folder on one machine"
            ),
            Self::Unavailable {
                package,
                need,
                reason,
            } => format!("{package}: {need}: {reason}"),
            Self::Bounds { limit, at } => match limit {
                BoundsLimit::Packages => {
                    format!("the dependency graph is over its package limit at `{at}`")
                }
                BoundsLimit::Bytes => {
                    format!("the dependency graph is over its size limit at `{at}`")
                }
            },
            Self::Oscillation { name, steps } => format!(
                "version conflict for `{name}`: {}",
                oscillation_detail(name, steps)
            ),
            Self::Frozen { message } => format!("frozen violation: {message}"),
        }
    }

    /// The `message` of the conflict-shaped error the CLI raises: the text after its prefix.
    pub fn conflict_detail(&self) -> Option<String> {
        match self {
            Self::Conflict { detail, .. } => Some(detail.clone()),
            Self::Oscillation { name, steps } => Some(oscillation_detail(name, steps)),
            _ => None,
        }
    }

    /// The `message` of an engine-unsatisfiable error: the text after its prefix.
    pub fn unsatisfiable_detail(&self) -> Option<String> {
        match self {
            Self::EngineUnsatisfiable { excluded, .. } => Some(unsatisfiable_detail(excluded)),
            _ => None,
        }
    }
}

pub(super) fn unsatisfiable_detail(excluded: &[(Version, Vec<EngineFailure>)]) -> String {
    let candidates = excluded
        .iter()
        .map(|(version, failures)| format!("  {version}: {}", describe(failures)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("no compatible candidate remains:\n{candidates}")
}

fn oscillation_detail(name: &str, steps: &[Step]) -> String {
    let cycle: Vec<String> = steps
        .iter()
        .map(|step| {
            format!(
                "{}@{}",
                step.version.as_deref().unwrap_or("no-version"),
                step.commit.as_deref().unwrap_or("no-commit")
            )
        })
        .collect();
    format!(
        "resolution oscillation detected for `{name}`: {}",
        cycle.join(" -> ")
    )
}
