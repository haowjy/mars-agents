//! The walk: one pass over the graph, restarted when a constraint changes a version already
//! chosen. Everything that must outlive a pass lives on [`Run`]; everything else is in [`Pass`] and
//! is rebuilt.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use indexmap::IndexMap;
use semver::Version;

use super::engine::{EngineFailure, describe};
use super::{
    Answer, Answers, BoundsLimit, Dependency, Diagnostic, EngineFallback, EngineFallbackSkipped,
    Failed, FailedFetch, Failure, Identity, Level, Need, Node, Origin, Outcome, PathRequest,
    PathSource, PluginRoot, Ref, Request, Resolved, Rooted, SourceId, Spec, Step as Oscillation,
    Tag, TagVersion,
};
use crate::diagnostic::SchemaDiagnostic;
use crate::inspect::{Hint, Kind, inspect};
use crate::normalize::{NormalizeRequest, Selection, SourceInfo, normalize};
use crate::prepare::Prepared;
use crate::tree::TreeSource;
use crate::version::{Constraint, satisfies, settle};

/// Why the walk left a branch.
pub(super) enum Stop {
    /// The run is over.
    Fail(Box<Failure>),
    /// A constraint changed a chosen version: walk again with it fixed.
    Restart,
    /// A restart wanted after an open need: the pass ends and the frontier is returned.
    EndPass,
    /// This branch waits on an answer, or failed after one that is open: go on with the others.
    Skip,
}

pub(super) type Flow<T> = Result<T, Stop>;

/// A version chosen for a source, kept across restarts so the walk does not choose it again. It
/// is used only for that source and only while it meets every constraint on it.
#[derive(Clone)]
pub(super) struct Chosen {
    pub id: SourceId,
    pub resolved: Ref,
    pub rooted: Rooted,
    pub plugin: Option<PluginRoot>,
    /// What choosing it reported (a stale lock row, an engine fallback): reused with the choice,
    /// so a restart that keeps the choice keeps what it said.
    pub notes: Vec<Diagnostic>,
}

#[derive(Default)]
struct Pass {
    registry: IndexMap<String, Node>,
    /// Names whose resolution began: one in here and not in the registry is waiting on an answer.
    started: BTreeSet<String>,
    ids: BTreeMap<SourceId, String>,
    constraints: BTreeMap<String, Vec<(String, Constraint)>>,
    notes: Vec<Diagnostic>,
    open: Vec<Need>,
    restart: Option<(String, Chosen)>,
}

/// The bytes of each file read, once per file per pass, whatever the number of rereads.
struct Counter {
    seen: BTreeSet<(String, String, String)>,
    bytes: u64,
    limit: Option<u64>,
    /// The package being walked: the bytes read now are its, so it is where the limit is crossed.
    reading: String,
    over: Option<String>,
}

impl Counter {
    fn new(limit: Option<u64>) -> Self {
        Self {
            seen: BTreeSet::new(),
            bytes: 0,
            limit,
            reading: String::new(),
            over: None,
        }
    }
}

pub(super) struct Run<'a> {
    pub(super) req: &'a Request,
    pub(super) answers: &'a dyn Answers,
    overrides: BTreeMap<SourceId, Chosen>,
    /// The refs each source has been fixed at, in order: one that comes back is an oscillation.
    history: BTreeMap<SourceId, Vec<Ref>>,
    /// Versions of a source its engine requirement rules out.
    pub(super) exclusions: BTreeMap<(SourceId, Version), Vec<EngineFailure>>,
    pass: Pass,
    counter: Rc<RefCell<Counter>>,
}

/// A tree that counts the bytes it hands out.
pub(super) struct Counted {
    inner: Rc<dyn TreeSource>,
    origin: (String, String),
    prefix: String,
    counter: Rc<RefCell<Counter>>,
}

impl TreeSource for Counted {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let bytes = self.inner.read(path)?;
        let full = if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", self.prefix)
        };
        let mut counter = self.counter.borrow_mut();
        if counter
            .seen
            .insert((self.origin.0.clone(), self.origin.1.clone(), full))
        {
            counter.bytes += bytes.len() as u64;
            if counter.over.is_none() && counter.limit.is_some_and(|limit| counter.bytes > limit) {
                counter.over = Some(counter.reading.clone());
            }
        }
        Some(bytes)
    }

    fn is_file(&self, path: &str) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.inner.is_dir(path)
    }

    fn children(&self, dir: &str) -> Vec<String> {
        self.inner.children(dir)
    }
}

impl<'a> Run<'a> {
    pub(super) fn new(req: &'a Request, answers: &'a dyn Answers) -> Self {
        Self {
            req,
            answers,
            overrides: BTreeMap::new(),
            history: BTreeMap::new(),
            exclusions: BTreeMap::new(),
            pass: Pass::default(),
            counter: Rc::new(RefCell::new(Counter::new(req.bounds.bytes))),
        }
    }

    pub(super) fn run(mut self) -> Outcome {
        loop {
            self.pass = Pass::default();
            *self.counter.borrow_mut() = Counter::new(self.req.bounds.bytes);
            match self.walk() {
                Ok(()) | Err(Stop::Skip | Stop::EndPass) => break,
                Err(Stop::Fail(failure)) => return self.failed(*failure),
                Err(Stop::Restart) => {
                    if let Some(failure) = self.restart() {
                        return self.failed(failure);
                    }
                }
            }
        }
        if !self.pass.open.is_empty() {
            let mut needs = std::mem::take(&mut self.pass.open);
            needs.sort();
            return Outcome::Needs(needs);
        }
        if let Some(failure) = self.bounds_exceeded() {
            return self.failed(failure);
        }
        self.resolved()
    }

    fn walk(&mut self) -> Flow<()> {
        let req = self.req;
        for dep in &req.root {
            self.branch(dep)?;
        }
        Ok(())
    }

    /// One dependency; a branch that waits does not stop its siblings.
    fn branch(&mut self, dep: &Dependency) -> Flow<()> {
        match self.resolve_package(dep) {
            Err(Stop::Skip) => Ok(()),
            other => other,
        }
    }

    fn failed(&mut self, failure: Failure) -> Outcome {
        Outcome::Failed(Failed {
            failure,
            diagnostics: std::mem::take(&mut self.pass.notes),
        })
    }

    /// Fix the version that changed and walk again, unless it is one the walk already tried.
    fn restart(&mut self) -> Option<Failure> {
        let (name, chosen) = self
            .pass
            .restart
            .take()
            .expect("a restart carries the version that caused it");
        let history = self.history.entry(chosen.id.clone()).or_default();
        if let Some(start) = history.iter().position(|seen| *seen == chosen.resolved) {
            let steps = history[start..]
                .iter()
                .chain(std::iter::once(&chosen.resolved))
                .map(|seen| Oscillation {
                    version: seen
                        .tag
                        .clone()
                        .or_else(|| seen.version.as_ref().map(ToString::to_string)),
                    commit: seen.commit.clone(),
                })
                .collect();
            return Some(Failure::Oscillation { name, steps });
        }
        history.push(chosen.resolved.clone());
        self.overrides.insert(chosen.id.clone(), chosen);
        None
    }

    fn resolved(mut self) -> Outcome {
        let nodes = std::mem::take(&mut self.pass.registry);
        let constraints = std::mem::take(&mut self.pass.constraints);
        drift_warnings(&nodes, &constraints, &mut self.pass.notes);
        let engine_fallbacks = self.engine_fallbacks(&nodes);
        Outcome::Resolved(Resolved {
            nodes,
            constraints,
            engine_fallbacks,
            diagnostics: std::mem::take(&mut self.pass.notes),
        })
    }

    /// Engine fallbacks are facts about the final graph, so they are read from the exclusions,
    /// which live across passes, for the packages the graph kept.
    fn engine_fallbacks(&self, nodes: &IndexMap<String, Node>) -> Vec<EngineFallback> {
        nodes
            .iter()
            .filter_map(|(name, node)| {
                let skipped: Vec<_> = self
                    .exclusions
                    .iter()
                    .filter(|((source, _), _)| *source == node.id)
                    .rev()
                    .map(|((_, version), failures)| EngineFallbackSkipped {
                        version: version.to_string(),
                        failures: failures.clone(),
                    })
                    .collect();
                (!skipped.is_empty()).then(|| EngineFallback {
                    source: name.clone(),
                    skipped,
                    selected_version: label(&node.resolved),
                })
            })
            .collect()
    }

    pub(super) fn current_package(&self) -> String {
        self.counter.borrow().reading.clone()
    }

    pub(super) fn constraints_of(&self, name: &str) -> Vec<(String, Constraint)> {
        self.pass.constraints.get(name).cloned().unwrap_or_default()
    }

    // ---- what a lookup came back with ----

    pub(super) fn note(&mut self, level: Level, code: &str, message: String, once: bool) {
        self.pass.notes.push(Diagnostic::Note {
            level,
            code: code.to_string(),
            message,
            once,
        });
    }

    /// A failure ends the run, unless an earlier need is open: it might sit on a branch a later
    /// restart drops, so it waits for the needs before it.
    pub(super) fn fail<T>(&mut self, failure: Failure) -> Flow<T> {
        if self.pass.open.is_empty() {
            Err(Stop::Fail(Box::new(failure)))
        } else {
            Err(Stop::Skip)
        }
    }

    fn open<T>(&mut self, need: Need) -> Flow<T> {
        if self.pass.open.is_empty()
            && let Some(failure) = self.bounds_exceeded()
        {
            return Err(Stop::Fail(Box::new(failure)));
        }
        if !self.pass.open.contains(&need) {
            self.pass.open.push(need);
        }
        Err(Stop::Skip)
    }

    fn unavailable<T>(&mut self, need: Need, reason: String) -> Flow<T> {
        let package = self.current_package();
        self.fail(Failure::Unavailable {
            package,
            need,
            reason,
        })
    }

    /// The first limit crossed, named at the package that crossed it.
    fn bounds_exceeded(&self) -> Option<Failure> {
        let bounds = &self.req.bounds;
        if let Some(limit) = bounds.packages
            && let Some((at, _)) = self.pass.registry.get_index(limit)
        {
            return Some(Failure::Bounds {
                limit: BoundsLimit::Packages,
                at: at.clone(),
            });
        }
        let at = self.counter.borrow().over.clone()?;
        Some(Failure::Bounds {
            limit: BoundsLimit::Bytes,
            at,
        })
    }

    /// A restart ends the pass when an answer is still open.
    fn restart_with(&mut self, name: String, chosen: Chosen) -> Stop {
        if self.pass.open.is_empty() {
            self.pass.restart = Some((name, chosen));
            Stop::Restart
        } else {
            Stop::EndPass
        }
    }

    pub(super) fn tags(&mut self, url: &str) -> Flow<Vec<TagVersion>> {
        let need = Need::Tags {
            url: url.to_string(),
        };
        match self.answers.tags(url) {
            None => self.open(need),
            Some(Answer::Found(tags)) => Ok(sort_tags(tags)),
            Some(Answer::Missing) => self.unavailable(need, "does not exist".to_string()),
            Some(Answer::Failed(failed)) => self.unavailable(need, failed.to_string()),
        }
    }

    pub(super) fn reference(&mut self, url: &str, name: &str) -> Flow<String> {
        let need = Need::Ref {
            url: url.to_string(),
            name: name.to_string(),
        };
        match self.answers.reference(url, name) {
            None => self.open(need),
            Some(Answer::Found(commit)) => Ok(commit),
            Some(Answer::Missing) => self.unavailable(need, "does not exist".to_string()),
            Some(Answer::Failed(failed)) => self.unavailable(need, failed.to_string()),
        }
    }

    /// The files of a commit; `None` is a commit the repository does not have.
    pub(super) fn tree(
        &mut self,
        url: &str,
        commit: &str,
        path: Option<&str>,
    ) -> Flow<Option<Rc<Counted>>> {
        let need = Need::Tree {
            url: url.to_string(),
            commit: commit.to_string(),
            path: path.map(str::to_string),
        };
        match self.answers.tree(url, commit, path) {
            None => self.open(need),
            Some(Answer::Found(inner)) => Ok(Some(self.counted(
                inner,
                (url.to_string(), commit.to_string()),
                path,
            ))),
            Some(Answer::Missing) => Ok(None),
            Some(Answer::Failed(FailedFetch::Fetch(reason))) => self.unavailable(need, reason),
            Some(Answer::Failed(FailedFetch::Subpath(detail))) => {
                let failure = Failure::Subpath {
                    name: self.current_package(),
                    subpath: path.unwrap_or_default().to_string(),
                    detail,
                };
                self.fail(failure)
            }
        }
    }

    pub(super) fn path_source(
        &mut self,
        dep: &Dependency,
        path: &str,
        subpath: Option<&str>,
        declared_in: Option<&Rooted>,
    ) -> Flow<(String, Rc<Counted>)> {
        let answered = self.answers.path(&PathRequest {
            name: &dep.name,
            path,
            subpath,
            declared_in,
        });
        match answered {
            Answer::Found(PathSource { canonical, tree }) => {
                let counted = self.counted(tree, (canonical.clone(), String::new()), subpath);
                Ok((canonical, counted))
            }
            Answer::Failed(FailedFetch::Subpath(detail)) => self.fail(Failure::Subpath {
                name: dep.name.clone(),
                subpath: subpath.unwrap_or_default().to_string(),
                detail,
            }),
            Answer::Missing | Answer::Failed(FailedFetch::Fetch(_)) => self.fail(Failure::Path {
                name: dep.name.clone(),
                declared_in: dep.required_by.clone(),
            }),
        }
    }

    fn counted(
        &self,
        inner: Rc<dyn TreeSource>,
        origin: (String, String),
        prefix: Option<&str>,
    ) -> Rc<Counted> {
        Rc::new(Counted {
            inner,
            origin,
            prefix: prefix.unwrap_or_default().to_string(),
            counter: Rc::clone(&self.counter),
        })
    }

    /// The tree of a package root, read through the same lookups a fetch would use.
    pub(super) fn rooted_tree(&mut self, dep: &Dependency, rooted: &Rooted) -> Flow<Rc<Counted>> {
        let subpath = rooted.subpath.as_deref();
        match &rooted.checkout {
            Origin::Git { url, commit } => match self.tree(url, commit, subpath)? {
                Some(tree) => Ok(tree),
                None => {
                    let need = Need::Tree {
                        url: url.clone(),
                        commit: commit.clone(),
                        path: subpath.map(str::to_string),
                    };
                    self.unavailable(need, "does not exist".to_string())
                }
            },
            Origin::Path { dir } => Ok(self.path_source(dep, dir, subpath, None)?.1),
        }
    }

    /// An error a tree met reading a file ends the run at the tree that was being read.
    pub(super) fn read_error(&mut self, rooted: &Rooted) -> Flow<()> {
        let Some(reason) = self.answers.take_read_error() else {
            return Ok(());
        };
        let need = match &rooted.checkout {
            Origin::Git { url, commit } => Need::Tree {
                url: url.clone(),
                commit: commit.clone(),
                path: rooted.subpath.clone(),
            },
            Origin::Path { dir } => Need::Tree {
                url: dir.clone(),
                commit: String::new(),
                path: rooted.subpath.clone(),
            },
        };
        self.unavailable(need, reason)
    }

    // ---- the walk ----

    fn identities(&mut self, dep: &Dependency) -> Flow<(SourceId, SourceId)> {
        let id = self.source_id(dep, &dep.spec)?;
        let declared = if dep.declared == dep.spec {
            id.clone()
        } else {
            match &dep.declared {
                // The override replaces a path nobody fetches, which need not exist: when it cannot
                // be canonicalized it is compared as written, as v0.34.5 did.
                Spec::Path(path) => {
                    let answered = self.answers.path(&PathRequest {
                        name: &dep.name,
                        path,
                        subpath: None,
                        declared_in: dep.declared_in.as_ref(),
                    });
                    SourceId::Path {
                        canonical: match answered {
                            Answer::Found(found) => found.canonical,
                            Answer::Missing | Answer::Failed(_) => path.clone(),
                        },
                        subpath: dep.subpath.clone(),
                        plugin: dep.plugin.clone(),
                    }
                }
                git => self.source_id(dep, git)?,
            }
        };
        Ok((declared, id))
    }

    fn source_id(&mut self, dep: &Dependency, spec: &Spec) -> Flow<SourceId> {
        Ok(match spec {
            Spec::Git { url, .. } => SourceId::Git {
                url: crate::git_url::canonicalize_git_url(url),
                subpath: dep.subpath.clone(),
                plugin: dep.plugin.clone(),
            },
            Spec::Path(path) => SourceId::Path {
                canonical: self
                    .path_source(dep, path, None, dep.declared_in.as_ref())?
                    .0,
                subpath: dep.subpath.clone(),
                plugin: dep.plugin.clone(),
            },
        })
    }

    fn resolve_package(&mut self, dep: &Dependency) -> Flow<()> {
        self.counter.borrow_mut().reading.clone_from(&dep.name);
        let (declared, id) = self.identities(dep)?;
        if let Some(existing) = self.pass.ids.get(&id)
            && existing != &dep.name
        {
            let failure = Failure::Identity(Identity::OneSourceTwoNames {
                existing: existing.clone(),
                duplicate: dep.name.clone(),
                source: id.to_string(),
            });
            return self.fail(failure);
        }
        if let Some(existing) = self.pass.registry.get(&dep.name)
            && existing.declared != declared
        {
            let failure = Failure::Identity(Identity::OneNameTwoSources {
                name: dep.name.clone(),
                existing: existing.declared.to_string(),
                incoming: declared.to_string(),
            });
            return self.fail(failure);
        }

        self.pass
            .constraints
            .entry(dep.name.clone())
            .or_default()
            .push((dep.required_by.clone(), dep.constraint.clone()));

        // A package reached again, by a sibling or by a cycle back to one whose subtree is still
        // being walked, is held to this constraint like any other request.
        if self.pass.registry.contains_key(&dep.name) {
            return self.recheck(dep, &id);
        }
        if !self.pass.started.insert(dep.name.clone()) {
            // Waiting on an answer: the constraint is recorded, and the walk comes back to it.
            return Ok(());
        }

        let pending = Pending { dep, id: &id };
        let mark = self.pass.notes.len();
        let mut rejected: Vec<(String, Vec<EngineFailure>)> = Vec::new();
        let mut override_candidate = self.reusable_choice(&id, &dep.name);
        let (resolved, rooted, plugin, prepared, normalized) = loop {
            let (resolved, rooted, plugin) = match override_candidate.take() {
                Some(chosen) => {
                    self.pass.notes.extend(chosen.notes);
                    (chosen.resolved, chosen.rooted, chosen.plugin)
                }
                None => match self.resolve_single_source(&pending) {
                    Ok(resolved) => {
                        let (rooted, plugin) = self.root_package(&pending, &resolved)?;
                        (resolved, rooted, plugin)
                    }
                    Err(super::version::SingleError::Conflict(conflict))
                        if !rejected.is_empty() =>
                    {
                        let failure = self.engine_unsatisfiable(&id, &dep.name, *conflict);
                        return self.fail(failure);
                    }
                    Err(error) => return Err(error.stop(self)),
                },
            };
            let (prepared, normalized) = self.prepare(dep, &rooted, plugin.as_ref())?;
            let failures = match package_of(&prepared) {
                Some(package) => match self.req.engine.check(package) {
                    Ok(failures) => failures,
                    Err(failure) => return self.fail(failure),
                },
                None => Vec::new(),
            };
            if failures.is_empty() {
                break (resolved, rooted, plugin, prepared, normalized);
            }

            let version_label = label(&resolved);
            if self.req.mode.is_frozen() {
                let message = format!(
                    "--frozen locked source `{}` version {version_label} is incompatible: {}",
                    dep.name,
                    describe(&failures)
                );
                return self.fail(Failure::Frozen { message });
            }
            let Some(version) = resolved.version.clone() else {
                let failure = Failure::EngineIncompatible {
                    name: dep.name.clone(),
                    failures: failures.clone(),
                    message: format!(
                        "version {version_label} requires {}; required by {}",
                        describe(&failures),
                        dep.required_by
                    ),
                };
                return self.fail(failure);
            };
            self.exclusions
                .insert((id.clone(), version), failures.clone());
            self.note(
                Level::Advisory,
                "requires-mars-fallback",
                format!(
                    "skipping `{}` {version_label}: {}; required by {}",
                    dep.name,
                    describe(&failures),
                    dep.required_by
                ),
                false,
            );
            rejected.push((version_label, failures));
        };
        if let Some((locked_version, failures)) = rejected.first()
            && self
                .req
                .locked
                .get(&dep.name)
                .and_then(|locked| locked.version.as_deref())
                .is_some_and(|version| version.trim_start_matches('v') == locked_version)
        {
            let message = format!(
                "locked `{}` {locked_version} is engine-incompatible ({}); selected {} instead",
                dep.name,
                describe(failures),
                label(&resolved)
            );
            self.note(
                Level::Advisory,
                "requires-mars-lock-fallback",
                message,
                false,
            );
        }
        if !rejected.is_empty() {
            let notes = self.pass.notes[mark..].to_vec();
            self.overrides.insert(
                id.clone(),
                Chosen {
                    id: id.clone(),
                    resolved: resolved.clone(),
                    rooted: rooted.clone(),
                    plugin: plugin.clone(),
                    notes,
                },
            );
        }

        let requests = self.manifest_requests(dep, &rooted, &prepared);
        self.pass.notes.push(Diagnostic::Registered {
            package: dep.name.clone(),
        });
        self.pass.registry.insert(
            dep.name.clone(),
            Node {
                name: dep.name.clone(),
                declared,
                id: id.clone(),
                fetch_url: match &dep.spec {
                    Spec::Git { url, .. } => Some(url.clone()),
                    Spec::Path(_) => None,
                },
                resolved,
                rooted,
                plugin,
                prepared,
                normalized,
            },
        );
        self.pass.ids.insert(id, dep.name.clone());

        for request in &requests {
            self.branch(request)?;
        }
        Ok(())
    }

    /// The choice kept for this source, when it still holds: its version is not excluded by the
    /// engine and it meets every constraint on the name now. A choice that does not is dropped,
    /// and the version is selected again.
    fn reusable_choice(&mut self, id: &SourceId, name: &str) -> Option<Chosen> {
        let chosen = self.overrides.get(id)?;
        let constraints = self.constraints_of(name);
        let requests: Vec<(&str, &Constraint)> = constraints
            .iter()
            .map(|(requester, constraint)| (requester.as_str(), constraint))
            .collect();
        let excluded =
            chosen.resolved.version.as_ref().is_some_and(|version| {
                self.exclusions.contains_key(&(id.clone(), version.clone()))
            });
        let holds = !excluded
            && match settle(&requests) {
                Ok(Some(reference)) => {
                    chosen.resolved.version.is_none()
                        && chosen.resolved.tag.as_deref() == Some(reference)
                }
                Ok(None) => match &chosen.resolved.version {
                    Some(version) => requests
                        .iter()
                        .all(|(_, constraint)| satisfies(constraint, version)),
                    None => !requests
                        .iter()
                        .any(|(_, constraint)| matches!(constraint, Constraint::Semver(_))),
                },
                Err(_) => false,
            };
        if holds {
            Some(chosen.clone())
        } else {
            self.overrides.remove(id);
            None
        }
    }

    /// A package already resolved is asked for again with another constraint. A git source may
    /// pick a different version under the constraints so far, which restarts the walk.
    fn recheck(&mut self, dep: &Dependency, id: &SourceId) -> Flow<()> {
        if !matches!(dep.spec, Spec::Git { .. }) {
            return Ok(());
        }
        let existing = self.pass.registry[&dep.name].resolved.clone();
        let satisfied = match (&dep.constraint, &existing.version) {
            (Constraint::Semver(req), Some(version)) => req.matches(version),
            _ => false,
        };
        if satisfied {
            return Ok(());
        }
        let pending = Pending { dep, id };
        let mark = self.pass.notes.len();
        let resolved = match self.resolve_single_source(&pending) {
            Ok(resolved) => resolved,
            Err(super::version::SingleError::Conflict(conflict)) => {
                let failure = self.engine_unsatisfiable(id, &dep.name, *conflict);
                return self.fail(failure);
            }
            Err(error) => return Err(error.stop(self)),
        };
        // Maximizing can select a different commit at the same version, or the same commit under
        // another selector, which the lock records and `--frozen` checks.
        if resolved == existing {
            return Ok(());
        }
        let (rooted, plugin) = self.root_package(&pending, &resolved)?;
        self.prepare(dep, &rooted, plugin.as_ref())?;
        let notes = self.pass.notes[mark..].to_vec();
        Err(self.restart_with(
            dep.name.clone(),
            Chosen {
                id: id.clone(),
                resolved,
                rooted,
                plugin,
                notes,
            },
        ))
    }

    /// A selection that found nothing, reported as the engine exclusions that emptied it when
    /// some excluded version meets every constraint, and as `conflict` itself otherwise.
    fn engine_unsatisfiable(&self, id: &SourceId, name: &str, conflict: Failure) -> Failure {
        let constraints = self.pass.constraints.get(name);
        let mut excluded: Vec<_> = self
            .exclusions
            .iter()
            .filter(|((source, version), _)| {
                source == id
                    && constraints.is_none_or(|constraints| {
                        constraints
                            .iter()
                            .all(|(_, constraint)| crate::version::satisfies(constraint, version))
                    })
            })
            .map(|((_, version), failures)| (version.clone(), failures.clone()))
            .collect();
        if excluded.is_empty() {
            return conflict;
        }
        excluded.sort_by(|(left, _), (right, _)| right.cmp(left));
        Failure::EngineUnsatisfiable {
            name: name.to_string(),
            excluded,
        }
    }

    /// Prepare a package's files, or take the preparation the request supplied for it.
    fn prepare(
        &mut self,
        dep: &Dependency,
        rooted: &Rooted,
        plugin: Option<&PluginRoot>,
    ) -> Flow<Preparation> {
        if let Some(supplied) = self.req.supplied.get(&dep.name) {
            return Ok((Arc::clone(supplied), BTreeMap::new()));
        }
        let tree = self.rooted_tree(dep, rooted)?;
        let hint = Hint {
            dialect: self
                .req
                .root
                .iter()
                .find(|root| root.name == dep.name)
                .and_then(|root| root.dialect),
            plugin: plugin.map(|plugin| plugin.hint.clone()),
            ..Hint::default()
        };
        let request = NormalizeRequest {
            tree: &*tree,
            source: SourceInfo {
                name: Some(dep.name.clone()),
                ..SourceInfo::default()
            },
            hint,
            selection: Selection::default(),
        };
        let mut normalized = normalize(&request);
        self.read_error(rooted)?;
        if normalized.kind == Kind::Marketplace {
            let failure = match plugin {
                // The plugin was chosen from its marketplace; what is wrong is the plugin itself,
                // which the crate refuses by reporting the root as a marketplace.
                Some(plugin) => Failure::Plugin {
                    name: dep.name.clone(),
                    detail: super::PluginDetail::Refused,
                    plugin: Some(plugin.name().to_string()),
                    text: format!(
                        "`{}`, plugin `{}`: {}",
                        dep.name,
                        plugin.name(),
                        normalized
                            .diagnostics
                            .values()
                            .flatten()
                            .find(|diagnostic| diagnostic.is_error())
                            .map_or("it cannot be imported", |refusal| refusal.message.as_str())
                    ),
                },
                None => {
                    let inspection = inspect(&*tree, &request.hint);
                    Failure::Plugin {
                        name: dep.name.clone(),
                        detail: super::PluginDetail::ChoiceNeeded,
                        plugin: None,
                        text: format!(
                            "`{}` is a marketplace with these plugins:\n{}\nname the one to install with `--plugin <name>` on `mars add`, or `plugin = \"<name>\"` on its dependency entry",
                            dep.name,
                            super::plugin::plugin_list(&inspection.plugins)
                        ),
                    }
                }
            };
            return self.fail(failure);
        }
        let [package] = normalized.packages.as_mut_slice() else {
            return self.fail(Failure::Plugin {
                name: dep.name.clone(),
                detail: super::PluginDetail::NotOnePackage,
                plugin: None,
                text: format!("`{}` did not normalize to exactly one package", dep.name),
            });
        };
        let prepared = Arc::new(package.prepared.clone());
        if let Some(manifest) = &prepared.manifest
            && manifest.reading.package.fields.is_none()
        {
            let invalid: Vec<_> = manifest
                .reading
                .package
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "toml.invalid")
                .cloned()
                .collect();
            if !invalid.is_empty() {
                return self.fail(Failure::Unreadable {
                    name: dep.name.clone(),
                    rooted: rooted.clone(),
                    diagnostics: invalid,
                });
            }
        }
        Ok((prepared, normalized.diagnostics))
    }

    /// The requests a package's manifest makes, with the local overrides applied by name.
    fn manifest_requests(
        &self,
        dep: &Dependency,
        rooted: &Rooted,
        prepared: &Prepared,
    ) -> Vec<Dependency> {
        let Some(manifest) = &prepared.manifest else {
            return Vec::new();
        };
        manifest
            .reading
            .dependencies
            .dependencies
            .iter()
            .map(|(name, entry)| {
                let declared = Spec::of(entry);
                let spec = self
                    .req
                    .overrides
                    .get(name)
                    .map_or_else(|| declared.clone(), |path| Spec::Path(path.clone()));
                Dependency {
                    name: name.clone(),
                    constraint: spec.constraint(),
                    declared,
                    spec,
                    subpath: entry.subpath.clone(),
                    plugin: entry.plugin.clone(),
                    required_by: dep.name.clone(),
                    declared_in: Some(rooted.clone()),
                    dialect: None,
                }
            })
            .collect()
    }
}

/// A package's preparation, and what normalizing its tree reported by file.
type Preparation = (Arc<Prepared>, BTreeMap<String, Vec<SchemaDiagnostic>>);

/// A dependency being resolved, with the identity its source has.
pub(super) struct Pending<'d> {
    pub dep: &'d Dependency,
    pub id: &'d SourceId,
}

fn package_of(prepared: &Prepared) -> Option<&crate::package::PackageMetadata> {
    prepared.manifest.as_ref()?.reading.package.fields.as_ref()
}

pub(super) fn label(resolved: &Ref) -> String {
    resolved
        .version
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| resolved.tag.clone())
        .unwrap_or_else(|| "HEAD/path".to_string())
}

/// The tags that are versions, oldest first; equal versions sort by tag, so the order of a
/// listing never changes the choice.
pub fn sort_tags(tags: Vec<Tag>) -> Vec<TagVersion> {
    let mut listed: Vec<TagVersion> = tags
        .into_iter()
        .filter_map(|tag| {
            Some(TagVersion {
                version: crate::version::parse_tag(&tag.tag)?,
                tag: tag.tag,
                commit: tag.commit,
            })
        })
        .collect();
    listed.sort_by(|a, b| (&a.version, &a.tag).cmp(&(&b.version, &b.tag)));
    listed
}

/// Warn once, on the settled graph, about a `Latest` request beside a pin: it holds, but asked
/// for the newest and got a fixed ref.
fn drift_warnings(
    nodes: &IndexMap<String, Node>,
    constraints: &BTreeMap<String, Vec<(String, Constraint)>>,
    notes: &mut Vec<Diagnostic>,
) {
    for (name, list) in constraints {
        let Some(node) = nodes.get(name) else {
            continue;
        };
        let Some(pin) = node
            .resolved
            .version
            .is_none()
            .then_some(node.resolved.tag.as_deref())
            .flatten()
        else {
            continue;
        };
        for (requester, constraint) in list {
            if matches!(constraint, Constraint::Latest) {
                notes.push(Diagnostic::Note {
                    level: Level::Warning,
                    code: "potential-version-drift".to_string(),
                    message: format!(
                        "potential version drift: package '{name}' requested as latest by {requester} but pinned to ref:{pin}"
                    ),
                    once: false,
                });
            }
        }
    }
}

impl super::Mode {
    pub(super) fn is_frozen(&self) -> bool {
        matches!(self, Self::Frozen)
    }
}
