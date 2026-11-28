//! Which version of which source, for a whole dependency graph.
//!
//! [`resolve`] is a pure function over [`Answers`]: it asks for the tags of a repository, what a
//! ref points at, the files of a commit, and never fetches. `mars sync` answers each question as
//! it is asked (so no question stays open and the order of work is the CLI's); Flow answers from
//! a map and gets back the questions still open ([`Outcome::Needs`]).
//!
//! A pass walks the graph from the root; a constraint that changes a version already chosen
//! restarts the walk with that choice fixed, and diagnostics belong to the last pass only.
//! Selection of items, hooks and installation are not here: the caller runs them on the graph
//! this returns.

mod engine;
mod failure;
mod lock;
mod package;
mod plugin;
mod version;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use indexmap::IndexMap;
use semver::Version;

use crate::dependency::{Entry, Source};
use crate::diagnostic::SchemaDiagnostic;
use crate::dialect::Dialect;
use crate::names::SourceSubpath;
use crate::prepare::Prepared;
use crate::tree::TreeSource;
use crate::version::Constraint;

pub use engine::{BadVersion, Engine, EngineFailure};
pub use failure::{BoundsLimit, Failure, Identity, PluginDetail, Requester, Step, SubpathDetail};
pub use lock::{LockedContent, LockedDependency, PluginRoot};
pub use package::sort_tags;

/// A tag of a repository and the commit it points at, peeled for an annotated tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub tag: String,
    pub commit: String,
}

/// A tag that is a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagVersion {
    pub version: Version,
    pub tag: String,
    pub commit: String,
}

/// What a lookup found. `Missing` is a commit or ref that does not exist, and the only answer
/// that drops a lock; `Failed` is any other error and always stops resolution.
#[derive(Debug, Clone)]
pub enum Answer<T> {
    Found(T),
    Missing,
    Failed(FailedFetch),
}

/// Why a lookup failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailedFetch {
    /// The fetch itself failed (network, auth, a limit, an unreadable file): its text.
    Fetch(String),
    /// A tree lookup below a repository's root found no folder there, or no folder that stays
    /// inside it: the dependency's `subpath` is wrong. Only a tree (or path) lookup fails this way.
    Subpath(SubpathDetail),
}

impl fmt::Display for FailedFetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(reason) => f.write_str(reason),
            Self::Subpath(detail) => write!(f, "the folder {}", detail.reason()),
        }
    }
}

/// A path dependency asked for: `path` as written (relative to `declared_in`'s root, or absolute),
/// narrowed to `subpath`.
#[derive(Debug, Clone)]
pub struct PathRequest<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub subpath: Option<&'a str>,
    pub declared_in: Option<&'a Rooted>,
}

/// An answered path dependency: its canonical directory, and a tree rooted at `subpath` below it.
#[derive(Clone)]
pub struct PathSource {
    pub canonical: String,
    pub tree: Rc<dyn TreeSource>,
}

/// Every lookup the resolver makes. A tree is read on demand and rooted at `path` (`None` is the
/// whole repository); a lookup with no answer yet is an open need ([`Outcome::Needs`]).
pub trait Answers {
    fn tags(&self, url: &str) -> Option<Answer<Vec<Tag>>>;
    fn reference(&self, url: &str, name: &str) -> Option<Answer<String>>;
    fn tree(
        &self,
        url: &str,
        commit: &str,
        path: Option<&str>,
    ) -> Option<Answer<Rc<dyn TreeSource>>>;
    fn path(&self, request: &PathRequest<'_>) -> Answer<PathSource>;
    /// The first error a tree met reading a file since the last call: a tree has no error type, so
    /// an answerer that can fail to read latches it.
    fn take_read_error(&self) -> Option<String> {
        None
    }
}

/// An answer the resolver is waiting for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    Tags {
        url: String,
    },
    Ref {
        url: String,
        name: String,
    },
    Tree {
        url: String,
        commit: String,
        path: Option<String>,
    },
}

impl fmt::Display for Need {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tags { url } => write!(f, "tags of {url}"),
            Self::Ref { url, name } => write!(f, "ref `{name}` of {url}"),
            Self::Tree { url, commit, path } => match path {
                Some(path) => write!(f, "files of {url} at {commit} under {path}"),
                None => write!(f, "files of {url} at {commit}"),
            },
        }
    }
}

/// Where a package's files are.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Git { url: String, commit: String },
    Path { dir: String },
}

/// A package root: `subpath` below a checkout.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rooted {
    pub checkout: Origin,
    pub subpath: Option<String>,
}

/// The identity of a source: the same one under two names, or one name for two, is refused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceId {
    Git {
        url: String,
        subpath: Option<SourceSubpath>,
        plugin: Option<String>,
    },
    Path {
        canonical: String,
        subpath: Option<SourceSubpath>,
        plugin: Option<String>,
    },
}

impl SourceId {
    pub fn plugin(&self) -> Option<&str> {
        match self {
            Self::Git { plugin, .. } | Self::Path { plugin, .. } => plugin.as_deref(),
        }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (subpath, plugin) = match self {
            Self::Git {
                url,
                subpath,
                plugin,
            } => {
                write!(f, "git:{url}")?;
                (subpath, plugin)
            }
            Self::Path {
                canonical,
                subpath,
                plugin,
            } => {
                write!(f, "path:{canonical}")?;
                (subpath, plugin)
            }
        };
        if let Some(subpath) = subpath {
            write!(f, "@{subpath}")?;
        }
        if let Some(plugin) = plugin {
            write!(f, " (plugin {plugin})")?;
        }
        Ok(())
    }
}

/// Where a dependency's package comes from, as an entry spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    Git {
        url: String,
        version: Option<String>,
    },
    Path(String),
}

impl Spec {
    /// The source an entry spells.
    pub fn of(entry: &Entry) -> Self {
        match &entry.source {
            Source::Git { url } => Self::Git {
                url: url.clone(),
                version: entry.version.clone(),
            },
            Source::Path { path } => Self::Path(path.to_string_lossy().into_owned()),
        }
    }

    pub fn constraint(&self) -> Constraint {
        match self {
            Self::Git { version, .. } => crate::version::parse_constraint(version.as_deref()),
            Self::Path(_) => Constraint::Latest,
        }
    }
}

/// One request for a source by name.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    /// The source the entry declares, before a local override.
    pub declared: Spec,
    /// The source fetched: `declared`, or an override's path.
    pub spec: Spec,
    pub subpath: Option<SourceSubpath>,
    pub plugin: Option<String>,
    pub constraint: Constraint,
    pub required_by: String,
    /// The package whose manifest declares this entry; `None` for the root's own.
    pub declared_in: Option<Rooted>,
    /// The inbound dialect the root's entry sets; read for the root's entries only.
    pub dialect: Option<Dialect>,
    /// Set on a root entry that brings in another root's graph whole (the user level's
    /// `_user-<name>`), never read from a manifest: every dependency its packages declare is
    /// named with this prefix and inherits it, so the subgraph never merges with this root's own
    /// packages, and each of its names replays its `locked` row exactly, as `--frozen` does,
    /// whatever `mode` says.
    pub namespace: Option<String>,
}

/// How a run treats the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Replay compatible lock entries, otherwise the newest compatible.
    Sync,
    /// The lock replays exactly, or the run fails.
    Frozen,
    /// Ignore the lock for `targets` (every name when empty).
    Upgrade { targets: BTreeSet<String> },
}

#[derive(Debug, Clone, Default)]
pub struct Bounds {
    pub packages: Option<usize>,
    pub bytes: Option<u64>,
}

pub struct Request {
    pub root: Vec<Dependency>,
    /// A package's preparation replaced, its source still answered.
    pub supplied: BTreeMap<String, Arc<Prepared>>,
    /// Local overrides by name: the path that replaces the source of every entry of that name.
    pub overrides: BTreeMap<String, String>,
    pub locked: BTreeMap<String, LockedDependency>,
    pub mode: Mode,
    pub engine: Engine,
    pub bounds: Bounds,
}

/// A package's resolved version: the tag it was selected at (`version` and `tag`), or a pinned
/// ref (`tag` only), or neither for a default branch or a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub version: Option<Version>,
    pub tag: Option<String>,
    pub commit: Option<String>,
    pub origin: Origin,
}

#[derive(Clone)]
pub struct Node {
    pub name: String,
    pub declared: SourceId,
    pub id: SourceId,
    /// The URL the source was fetched with, as the entry spelled it; `None` for a path.
    pub fetch_url: Option<String>,
    pub resolved: Ref,
    /// Where the package's files are: the marketplace's own checkout for a local plugin, the
    /// plugin's repository for a remote one.
    pub rooted: Rooted,
    pub plugin: Option<PluginRoot>,
    pub prepared: Arc<Prepared>,
    /// What normalizing the tree reported before the package was split out, by file.
    pub normalized: BTreeMap<String, Vec<SchemaDiagnostic>>,
}

/// The `mars.lock` row of a dependency resolved at `commit` (under `tag`, when it has one).
pub fn locked_dependency(
    id: &SourceId,
    tag: Option<&str>,
    commit: Option<&str>,
    plugin: Option<&PluginRoot>,
) -> LockedDependency {
    let (url, path, subpath) = match id {
        SourceId::Git { url, subpath, .. } => (Some(url.clone()), None, subpath.clone()),
        SourceId::Path {
            canonical, subpath, ..
        } => (None, Some(canonical.clone()), subpath.clone()),
    };
    LockedDependency {
        url,
        path,
        subpath,
        version: tag.map(str::to_owned),
        commit: commit.map(str::to_owned),
        plugin: plugin.and_then(|plugin| plugin.locked_name().map(str::to_owned)),
        content: plugin.and_then(|plugin| plugin.locked_content().cloned()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFallbackSkipped {
    pub version: String,
    pub failures: Vec<EngineFailure>,
}

/// A source that took an older version because a newer one needs a newer engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFallback {
    pub source: String,
    pub skipped: Vec<EngineFallbackSkipped>,
    pub selected_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The user loses something: the sync fell short of the state it wanted.
    Warning,
    /// A warning that does not imply it did.
    Advisory,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    Note {
        level: Level,
        code: String,
        message: String,
        /// Reported once however many passes or packages repeat it.
        once: bool,
    },
    /// The package was registered here: what preparing its files found belongs at this point, and
    /// is read from the node's `prepared` and `normalized`, which the consumer frames.
    Registered { package: String },
}

pub struct Resolved {
    /// In the order the final pass registered them.
    pub nodes: IndexMap<String, Node>,
    /// Every requester's constraint on each name.
    pub constraints: BTreeMap<String, Vec<(String, Constraint)>>,
    pub engine_fallbacks: Vec<EngineFallback>,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct Failed {
    pub failure: Failure,
    pub diagnostics: Vec<Diagnostic>,
}

pub enum Outcome {
    /// Answers still needed, every one on the frontier.
    Needs(Vec<Need>),
    Resolved(Resolved),
    /// The first failure; one found after an open need waits for the needs before it.
    Failed(Failed),
}

/// Resolve `request`, asking `answers`.
pub fn resolve(request: &Request, answers: &dyn Answers) -> Outcome {
    package::Run::new(request, answers).run()
}

#[cfg(test)]
mod tests;
