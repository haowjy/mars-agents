//! `resolve` as Flow calls it: JSON in, JSON out, answers from a list.
//!
//! [`run`] turns the answers in an [`Input`] into a map-backed [`Answers`] (a lookup with no
//! answer is an open need) and [`Output`] is the outcome in the shape `npm/@meridian-flow/mars-schema`
//! documents. Flow keeps no state between calls: every call replays the resolution from the full
//! list of answers.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use serde_bytes::ByteBuf;

use super::{
    Answer, Answers, Bounds, BoundsLimit, Dependency, Diagnostic, Engine, EngineFailure, Failure,
    Identity, Level, LockedDependency, Mode, Need, Outcome, PathRequest, PathSource, PluginDetail,
    Request, Spec, SubpathDetail, Tag, locked_dependency,
};
use crate::dependency::Dependencies;
use crate::diagnostic::SchemaDiagnostic;
use crate::names::SourceSubpath;
use crate::tree::{FileTree, TreeSource};
use crate::version::Constraint;

/// The root's name for itself in `required_by`; the core says `mars.toml`.
const ROOT_CORE: &str = "mars.toml";
const ROOT: &str = "root";

/// An argument that is not the shape `resolve` documents.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InputError(String);

impl InputError {
    pub fn code(&self) -> &'static str {
        "input.invalid"
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    root: RootInput,
    /// A package's preparation replaced by an edited manifest. Not supported yet: a non-empty
    /// map is refused.
    #[serde(default)]
    supplied: BTreeMap<String, SuppliedInput>,
    #[serde(default)]
    locked: BTreeMap<String, LockedDependency>,
    #[serde(default)]
    refresh: Refresh,
    engine: EngineInput,
    #[serde(default)]
    bounds: BoundsInput,
    answers: Vec<AnswerInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootInput {
    manifest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuppliedInput {
    #[allow(dead_code)]
    manifest: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
enum Refresh {
    #[default]
    None,
    All(RefreshAll),
    Names(Vec<String>),
}

#[derive(Debug, Deserialize)]
enum RefreshAll {
    #[serde(rename = "all")]
    All,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineInput {
    mars: String,
    #[serde(default)]
    meridian: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundsInput {
    #[serde(default)]
    packages: Option<usize>,
    #[serde(default)]
    bytes: Option<u64>,
}

/// A fetch asked for, or one answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NeedWire {
    Tags {
        url: String,
    },
    Ref {
        url: String,
        #[serde(rename = "ref")]
        name: String,
    },
    Tree {
        url: String,
        commit: String,
        path: Option<String>,
    },
}

impl From<Need> for NeedWire {
    fn from(need: Need) -> Self {
        match need {
            Need::Tags { url } => Self::Tags { url },
            Need::Ref { url, name } => Self::Ref { url, name },
            Need::Tree { url, commit, path } => Self::Tree { url, commit, path },
        }
    }
}

#[derive(Debug, Deserialize)]
struct TagInput {
    tag: String,
    commit: String,
}

/// One answer, in whichever of the five spellings: tags, a ref's commit, a tree's files, or a
/// need that is `missing` or `failed`.
#[derive(Debug, Deserialize)]
struct AnswerInput {
    need: Option<NeedWire>,
    tags: Option<Vec<TagInput>>,
    commit: Option<String>,
    url: Option<String>,
    #[serde(default, deserialize_with = "present")]
    path: Option<Option<String>>,
    files: Option<BTreeMap<String, ByteBuf>>,
    missing: Option<bool>,
    failed: Option<String>,
}

/// A key that is there, `null` included, apart from one that is not.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

/// What `resolve` returned.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Output {
    Needs {
        needs: Vec<NeedWire>,
    },
    Resolved {
        resolved: ResolvedOutput,
        diagnostics: Vec<DiagnosticOutput>,
    },
    Failed {
        failed: FailureOutput,
        diagnostics: Vec<DiagnosticOutput>,
    },
}

#[derive(Debug, Serialize)]
pub struct ResolvedOutput {
    lock: BTreeMap<String, LockedDependency>,
    dependencies: BTreeMap<String, DependencyOutput>,
}

#[derive(Debug, Serialize)]
struct DependencyOutput {
    url: String,
    commit: String,
    subpath: Option<SourceSubpath>,
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ContentOutput>,
    required_by: Vec<RequesterOutput>,
}

/// Where a plugin's files are, with every field spelled, unlike the lock's row.
#[derive(Debug, Serialize)]
struct ContentOutput {
    url: String,
    subpath: Option<SourceSubpath>,
    commit: String,
}

#[derive(Debug, Serialize)]
struct RequesterOutput {
    by: String,
    constraint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticOutput {
    level: &'static str,
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
pub struct FailureOutput {
    #[serde(flatten)]
    body: FailureBody,
    message: String,
}

#[derive(Debug, Serialize)]
struct ExcludedOutput {
    version: String,
    requirement: String,
    engine: &'static str,
}

#[derive(Debug, Serialize)]
struct StepOutput {
    version: Option<String>,
    commit: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum FailureBody {
    Conflict {
        name: String,
        requesters: Vec<RequesterOutput>,
    },
    Identity {
        detail: &'static str,
        names: Vec<String>,
        source: String,
        /// For `one-name-two-sources`: the source the name had first.
        #[serde(skip_serializing_if = "Option::is_none")]
        existing: Option<String>,
    },
    EngineIncompatible {
        name: String,
        requirement: Option<String>,
        engine: Option<&'static str>,
    },
    EngineUnsatisfiable {
        name: String,
        excluded: Vec<ExcludedOutput>,
    },
    EngineRequirementInvalid {
        name: String,
        requirement: String,
        engine: String,
    },
    EngineVersionInvalid {
        engine: String,
        version: String,
    },
    Plugin {
        name: String,
        detail: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        plugin: Option<String>,
    },
    Subpath {
        name: String,
        subpath: String,
        detail: &'static str,
    },
    Unreadable {
        name: String,
        diagnostics: Vec<SchemaDiagnostic>,
    },
    Path {
        name: String,
        declared_in: String,
    },
    Unavailable {
        need: NeedWire,
        reason: String,
    },
    Bounds {
        limit: &'static str,
        at: String,
    },
    Oscillation {
        name: String,
        steps: Vec<StepOutput>,
    },
    /// Only `--frozen` produces it, which `resolve` never runs.
    Frozen,
}

/// An answer to one kind of lookup, or why there is none.
enum Reply<T> {
    Found(T),
    Missing,
    Failed(String),
}

struct TreeReply {
    url: String,
    commit: String,
    /// The folder the answer covers, `None` for the whole repository.
    path: Option<String>,
    reply: Reply<Rc<FileTree>>,
}

/// The tree below a folder of an answer, or why the folder is not one.
type View = Result<Rc<FileTree>, SubpathDetail>;

/// The answers of an input as a map, which the resolver looks its lookups up in.
struct MapAnswers {
    tags: BTreeMap<String, Reply<Vec<Tag>>>,
    refs: BTreeMap<(String, String), Reply<String>>,
    trees: Vec<TreeReply>,
    /// Trees rooted below a repository's root, built once per lookup.
    views: RefCell<BTreeMap<(usize, String), View>>,
    /// Why a subtree lookup failed, so the failure it ends in says `subpath`, not `unavailable`.
    subpath_errors: RefCell<BTreeMap<(String, String, String), SubpathDetail>>,
}

fn invalid(message: impl Into<String>) -> InputError {
    InputError(message.into())
}

impl MapAnswers {
    fn new(answers: Vec<AnswerInput>) -> Result<Self, InputError> {
        let mut map = Self {
            tags: BTreeMap::new(),
            refs: BTreeMap::new(),
            trees: Vec::new(),
            views: RefCell::default(),
            subpath_errors: RefCell::default(),
        };
        for (index, answer) in answers.into_iter().enumerate() {
            map.add(answer)
                .map_err(|error| invalid(format!("answers[{index}]: {error}")))?;
        }
        Ok(map)
    }

    fn add(&mut self, answer: AnswerInput) -> Result<(), InputError> {
        if let Some(files) = answer.files {
            if answer.need.is_some()
                || answer.tags.is_some()
                || answer.missing.is_some()
                || answer.failed.is_some()
            {
                return Err(invalid(
                    "a tree answer carries only url, commit, path and files",
                ));
            }
            let (Some(url), Some(commit), Some(path)) = (answer.url, answer.commit, answer.path)
            else {
                return Err(invalid("a tree answer needs url, commit and path"));
            };
            let tree = FileTree::from_files(
                files
                    .into_iter()
                    .map(|(path, bytes)| (path, bytes.into_vec())),
            )
            .map_err(|error| invalid(error.to_string()))?;
            self.trees.push(TreeReply {
                url,
                commit,
                path,
                reply: Reply::Found(Rc::new(tree)),
            });
            return Ok(());
        }
        let Some(need) = answer.need else {
            return Err(invalid(
                "an answer names its `need`, or carries a tree's `files`",
            ));
        };
        let missing = answer.missing == Some(true);
        let given = [
            answer.tags.is_some(),
            answer.commit.is_some(),
            missing,
            answer.failed.is_some(),
        ]
        .into_iter()
        .filter(|given| *given)
        .count();
        if given != 1 || answer.missing == Some(false) {
            return Err(invalid(
                "an answer carries exactly one of `tags`, `commit`, `missing: true` or `failed`",
            ));
        }
        match need {
            NeedWire::Tags { url } => {
                let reply = match (answer.tags, answer.failed) {
                    (Some(tags), _) => Reply::Found(
                        tags.into_iter()
                            .map(|tag| Tag {
                                tag: tag.tag,
                                commit: tag.commit,
                            })
                            .collect(),
                    ),
                    (None, Some(reason)) => Reply::Failed(reason),
                    _ if missing => Reply::Missing,
                    _ => return Err(invalid("a tags need is answered with `tags`")),
                };
                self.tags.insert(url, reply);
            }
            NeedWire::Ref { url, name } => {
                let reply = match (answer.commit, answer.failed) {
                    (Some(commit), _) => Reply::Found(commit),
                    (None, Some(reason)) => Reply::Failed(reason),
                    _ if missing => Reply::Missing,
                    _ => return Err(invalid("a ref need is answered with a `commit`")),
                };
                self.refs.insert((url, name), reply);
            }
            NeedWire::Tree { url, commit, path } => {
                let reply = match answer.failed {
                    Some(reason) => Reply::Failed(reason),
                    None if missing => Reply::Missing,
                    None => {
                        return Err(invalid(
                            "a tree need is answered with `files`, `missing` or `failed`",
                        ));
                    }
                };
                self.trees.push(TreeReply {
                    url,
                    commit,
                    path,
                    reply,
                });
            }
        }
        Ok(())
    }

    /// The files of `answer` under `path`, as a tree rooted there.
    fn view(&self, index: usize, tree: &FileTree, path: &str) -> View {
        let key = (index, path.to_string());
        if let Some(view) = self.views.borrow().get(&key) {
            return view.clone();
        }
        let prefix = format!("{path}/");
        let view = if tree.is_file(path) {
            Err(SubpathDetail::NotADirectory)
        } else if !tree.is_dir(path) {
            Err(SubpathDetail::Missing)
        } else {
            let files = tree.files().iter().filter_map(|(file, bytes)| {
                file.strip_prefix(&prefix)
                    .map(|relative| (relative.to_string(), bytes.clone()))
            });
            Ok(Rc::new(
                FileTree::from_files(files).expect("paths of a tree stay valid below a folder"),
            ))
        };
        self.views.borrow_mut().insert(key, view.clone());
        view
    }
}

/// Whether the folder an answer covers (`None`: everything) holds `path`.
fn covers(answer: Option<&str>, path: Option<&str>) -> bool {
    match (answer, path) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(answer), Some(path)) => {
            path == answer
                || path
                    .strip_prefix(answer)
                    .is_some_and(|rest| rest.starts_with('/'))
        }
    }
}

impl<T: Clone> Reply<T> {
    fn answer(&self) -> Answer<T> {
        match self {
            Self::Found(value) => Answer::Found(value.clone()),
            Self::Missing => Answer::Missing,
            Self::Failed(reason) => Answer::Failed(reason.clone()),
        }
    }
}

impl Answers for MapAnswers {
    fn tags(&self, url: &str) -> Option<Answer<Vec<Tag>>> {
        self.tags.get(url).map(Reply::answer)
    }

    fn reference(&self, url: &str, name: &str) -> Option<Answer<String>> {
        self.refs
            .get(&(url.to_string(), name.to_string()))
            .map(Reply::answer)
    }

    fn tree(
        &self,
        url: &str,
        commit: &str,
        path: Option<&str>,
    ) -> Option<Answer<Rc<dyn TreeSource>>> {
        let (index, covering) = self.trees.iter().enumerate().find(|(_, tree)| {
            tree.url == url && tree.commit == commit && covers(tree.path.as_deref(), path)
        })?;
        let tree = match (&covering.reply, path) {
            (Reply::Found(tree), None) => Rc::clone(tree) as Rc<dyn TreeSource>,
            (Reply::Found(tree), Some(path)) => match self.view(index, tree, path) {
                Ok(view) => view as Rc<dyn TreeSource>,
                Err(detail) => {
                    let reason = match detail {
                        SubpathDetail::NotADirectory => format!("`{path}` is not a directory"),
                        _ => format!("`{path}` does not exist"),
                    };
                    self.subpath_errors.borrow_mut().insert(
                        (url.to_string(), commit.to_string(), path.to_string()),
                        detail,
                    );
                    return Some(Answer::Failed(reason));
                }
            },
            (Reply::Missing, _) => return Some(Answer::Missing),
            (Reply::Failed(reason), _) => return Some(Answer::Failed(reason.clone())),
        };
        Some(Answer::Found(tree))
    }

    /// A fetched package's path names its author's disk.
    fn path(&self, _request: &PathRequest<'_>) -> Answer<PathSource> {
        Answer::Failed("path refused".to_string())
    }
}

fn root_dependencies(manifest: &str) -> Result<Vec<Dependency>, InputError> {
    let document = manifest
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| invalid(format!("root.manifest is not TOML: {error}")))?;
    let mut diagnostics = Vec::new();
    let dependencies = Dependencies::read(&document, &mut diagnostics);
    if let Some(error) = diagnostics.iter().find(|diagnostic| diagnostic.is_error()) {
        return Err(invalid(format!("root.manifest: {}", error.message)));
    }
    Ok(dependencies
        .dependencies
        .into_iter()
        .map(|(name, entry)| {
            let spec = Spec::of(&entry);
            Dependency {
                name,
                constraint: spec.constraint(),
                declared: spec.clone(),
                spec,
                subpath: entry.subpath,
                plugin: entry.plugin,
                required_by: ROOT_CORE.to_string(),
                declared_in: None,
                dialect: entry.dialect,
            }
        })
        .collect())
}

/// Resolve what `input` answers, or say what it still needs.
pub fn run(input: Input) -> Result<Output, InputError> {
    if !input.supplied.is_empty() {
        return Err(invalid(
            "`supplied` is not supported yet: resolve prepares every package from its fetched tree",
        ));
    }
    let root = root_dependencies(&input.root.manifest)?;
    let answers = MapAnswers::new(input.answers)?;
    let mars = match Version::parse(&input.engine.mars) {
        Ok(mars) => mars,
        Err(error) => {
            let failure = Failure::EngineVersionInvalid {
                engine: "mars".to_string(),
                version: input.engine.mars,
                message: error.to_string(),
            };
            return Ok(failed(failure, &answers, &[]));
        }
    };
    let request = Request {
        root,
        supplied: BTreeMap::new(),
        overrides: BTreeMap::new(),
        locked: input.locked,
        mode: match input.refresh {
            Refresh::None => Mode::Sync,
            Refresh::All(_) => Mode::Upgrade {
                targets: BTreeSet::new(),
            },
            Refresh::Names(names) if names.is_empty() => Mode::Sync,
            Refresh::Names(names) => Mode::Upgrade {
                targets: names.into_iter().collect(),
            },
        },
        engine: Engine {
            mars,
            meridian: input.engine.meridian.map(|version| {
                Version::parse(&version).map_err(|error| super::BadVersion {
                    version,
                    message: error.to_string(),
                })
            }),
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
        },
        bounds: Bounds {
            packages: input.bounds.packages,
            bytes: input.bounds.bytes,
        },
    };
    Ok(match super::resolve(&request, &answers) {
        Outcome::Needs(needs) => Output::Needs {
            needs: needs.into_iter().map(NeedWire::from).collect(),
        },
        Outcome::Failed(error) => failed(error.failure, &answers, &error.diagnostics),
        Outcome::Resolved(resolved) => Output::Resolved {
            resolved: ResolvedOutput {
                lock: resolved
                    .nodes
                    .iter()
                    .map(|(name, node)| {
                        (
                            name.clone(),
                            locked_dependency(
                                &node.id,
                                node.resolved.tag.as_deref(),
                                node.resolved.commit.as_deref(),
                                node.plugin.as_ref(),
                            ),
                        )
                    })
                    .collect(),
                dependencies: dependencies(&resolved),
            },
            diagnostics: diagnostics(&resolved.diagnostics),
        },
    })
}

fn dependencies(resolved: &super::Resolved) -> BTreeMap<String, DependencyOutput> {
    resolved
        .nodes
        .iter()
        .filter_map(|(name, node)| {
            let lock = locked_dependency(
                &node.id,
                node.resolved.tag.as_deref(),
                node.resolved.commit.as_deref(),
                node.plugin.as_ref(),
            );
            // Path sources are refused, so a resolved node is a git one.
            let (url, commit) = (lock.url?, lock.commit?);
            let required_by = resolved
                .constraints
                .get(name)
                .into_iter()
                .flatten()
                .map(|(by, constraint)| RequesterOutput {
                    by: requester(by),
                    constraint: constraint_text(constraint),
                })
                .collect();
            Some((
                name.clone(),
                DependencyOutput {
                    url,
                    commit,
                    subpath: lock.subpath,
                    version: node.resolved.version.as_ref().and(lock.version),
                    content: lock.content.and_then(|content| {
                        Some(ContentOutput {
                            url: content.url?,
                            subpath: content.subpath,
                            commit: content.commit?,
                        })
                    }),
                    required_by,
                },
            ))
        })
        .collect()
}

fn requester(by: &str) -> String {
    if by == ROOT_CORE { ROOT } else { by }.to_string()
}

fn constraint_text(constraint: &Constraint) -> Option<String> {
    match constraint {
        Constraint::Latest => None,
        other => Some(other.to_string()),
    }
}

fn diagnostics(notes: &[Diagnostic]) -> Vec<DiagnosticOutput> {
    let mut seen = BTreeSet::new();
    notes
        .iter()
        .filter_map(|note| {
            let Diagnostic::Note {
                level,
                code,
                message,
                once,
            } = note
            else {
                return None;
            };
            if *once && !seen.insert((code.clone(), message.clone())) {
                return None;
            }
            Some(DiagnosticOutput {
                level: match level {
                    Level::Warning => "warning",
                    Level::Advisory => "advisory",
                    Level::Info => "info",
                },
                code: code.clone(),
                message: message.clone(),
            })
        })
        .collect()
}

fn failed(failure: Failure, answers: &MapAnswers, notes: &[Diagnostic]) -> Output {
    let message = failure.message();
    let body = match failure {
        Failure::Conflict {
            name, requesters, ..
        } => FailureBody::Conflict {
            name,
            requesters: requesters
                .into_iter()
                .map(|found| RequesterOutput {
                    by: requester(&found.by),
                    constraint: (found.constraint != "latest").then_some(found.constraint),
                })
                .collect(),
        },
        Failure::Identity(Identity::OneSourceTwoNames {
            existing,
            duplicate,
            source,
        }) => FailureBody::Identity {
            detail: "one-source-two-names",
            names: vec![existing, duplicate],
            source,
            existing: None,
        },
        Failure::Identity(Identity::OneNameTwoSources {
            name,
            existing,
            incoming,
        }) => FailureBody::Identity {
            detail: "one-name-two-sources",
            names: vec![name],
            source: incoming,
            existing: Some(existing),
        },
        Failure::EngineIncompatible { name, failures, .. } => {
            let first = failures.first();
            FailureBody::EngineIncompatible {
                name,
                requirement: first.map(|failure| failure.requirement.clone()),
                engine: first.map(|failure| failure.engine),
            }
        }
        Failure::EngineUnsatisfiable { name, excluded } => FailureBody::EngineUnsatisfiable {
            name,
            excluded: excluded
                .into_iter()
                .flat_map(|(version, failures)| {
                    failures.into_iter().map(
                        move |EngineFailure {
                                  engine,
                                  requirement,
                                  ..
                              }| ExcludedOutput {
                            version: version.to_string(),
                            requirement,
                            engine,
                        },
                    )
                })
                .collect(),
        },
        Failure::EngineRequirementInvalid {
            name,
            engine,
            requirement,
            ..
        } => FailureBody::EngineRequirementInvalid {
            name,
            requirement,
            engine,
        },
        Failure::EngineVersionInvalid {
            engine, version, ..
        } => FailureBody::EngineVersionInvalid { engine, version },
        Failure::Plugin {
            name,
            detail,
            plugin,
            ..
        } => FailureBody::Plugin {
            name,
            detail: match detail {
                PluginDetail::NotAMarketplace => "not-a-marketplace",
                PluginDetail::NotListed => "not-listed",
                PluginDetail::Refused => "refused",
                PluginDetail::ChoiceNeeded => "choice-needed",
                PluginDetail::NotOnePackage => "not-one-package",
                PluginDetail::UnusablePath => "unusable-path",
            },
            plugin,
        },
        Failure::Subpath {
            name,
            subpath,
            detail,
        } => FailureBody::Subpath {
            name,
            subpath,
            detail: subpath_detail(detail),
        },
        Failure::Unreadable {
            name, diagnostics, ..
        } => FailureBody::Unreadable { name, diagnostics },
        Failure::Path { name, declared_in } => FailureBody::Path {
            name,
            declared_in: requester(&declared_in),
        },
        Failure::Unavailable {
            package,
            need,
            reason,
        } => {
            // A folder the answer has no files in is the package's `subpath`.
            let subpath = match &need {
                Need::Tree {
                    url,
                    commit,
                    path: Some(path),
                } => answers
                    .subpath_errors
                    .borrow()
                    .get(&(url.clone(), commit.clone(), path.clone()))
                    .map(|detail| (path.clone(), *detail)),
                _ => None,
            };
            if let Some((subpath, detail)) = subpath {
                let failure = Failure::Subpath {
                    name: package,
                    subpath,
                    detail,
                };
                return failed(failure, answers, notes);
            }
            FailureBody::Unavailable {
                need: need.into(),
                reason,
            }
        }
        Failure::Bounds { limit, at } => FailureBody::Bounds {
            limit: match limit {
                BoundsLimit::Packages => "packages",
                BoundsLimit::Bytes => "bytes",
            },
            at,
        },
        Failure::Oscillation { name, steps } => FailureBody::Oscillation {
            name,
            steps: steps
                .into_iter()
                .map(|step| StepOutput {
                    version: step.version,
                    commit: step.commit,
                })
                .collect(),
        },
        Failure::Frozen { .. } => FailureBody::Frozen,
    };
    Output::Failed {
        failed: FailureOutput { body, message },
        diagnostics: diagnostics(notes),
    }
}

fn subpath_detail(detail: SubpathDetail) -> &'static str {
    match detail {
        SubpathDetail::Missing => "missing",
        SubpathDetail::NotADirectory => "not-a-directory",
        SubpathDetail::Traversal => "traversal",
    }
}
