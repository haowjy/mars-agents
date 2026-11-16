//! The CLI's answers to `mars_schema::resolve`: each lookup is fetched when it is asked and
//! memoized, so no need is ever open and the order of work is the order the core asks in.
//!
//! The core knows an answer failed and its reason's text; the error that failed it stays here, per
//! lookup, and [`CliAnswers::error_for`] hands it back when the core reports that lookup, so a
//! failure prints what it printed before the resolver moved. A wrong `subpath` is the exception:
//! the answer says so, and the core reports it as the failure it is.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mars_schema::names::SourceSubpath;
use mars_schema::resolve::{
    Answer, Answers, FailedFetch, Failure, Identity, Need, Origin, PathRequest, PathSource, Rooted,
    SubpathDetail, Tag,
};
use mars_schema::tree::TreeSource;

use super::SourceProvider;
use crate::error::{ConfigError, MarsError, ResolutionError};
use crate::package_prepare::DiskTreeSource;
use crate::types::SourceUrl;

/// Which lookup an error belongs to.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Lookup {
    Tags(String),
    Ref(String, String),
    Content(String, String),
    Tree(String, String, Option<String>),
    Path(String),
}

type TreeKey = (String, String, Option<String>);
type PathKey = (String, String, Option<String>, Option<Rooted>);

pub(crate) struct CliAnswers<'a> {
    provider: &'a dyn SourceProvider,
    tags: RefCell<BTreeMap<String, Answer<Vec<Tag>>>>,
    refs: RefCell<BTreeMap<(String, String), Answer<String>>>,
    content: RefCell<BTreeMap<(String, String), Answer<PathBuf>>>,
    /// The tag a commit was listed under: git fetches it by name.
    tag_of: RefCell<BTreeMap<(String, String), String>>,
    trees: RefCell<BTreeMap<TreeKey, Answer<Rc<dyn TreeSource>>>>,
    paths: RefCell<BTreeMap<PathKey, Answer<PathSource>>>,
    /// Every tree handed out, for the error one of them latched reading a file.
    disk: RefCell<Vec<Rc<DiskTreeSource>>>,
    errors: RefCell<BTreeMap<Lookup, MarsError>>,
    read_error: RefCell<Option<MarsError>>,
}

impl<'a> CliAnswers<'a> {
    pub(crate) fn new(provider: &'a dyn SourceProvider) -> Self {
        Self {
            provider,
            tags: RefCell::default(),
            refs: RefCell::default(),
            content: RefCell::default(),
            tag_of: RefCell::default(),
            trees: RefCell::default(),
            paths: RefCell::default(),
            disk: RefCell::default(),
            errors: RefCell::default(),
            read_error: RefCell::default(),
        }
    }

    fn failed<T>(&self, lookup: Lookup, error: MarsError) -> Answer<T> {
        let reason = error.to_string();
        self.errors.borrow_mut().insert(lookup, error);
        Answer::Failed(FailedFetch::Fetch(reason))
    }

    /// A folder that is not there, not a directory or outside the checkout is a wrong `subpath`,
    /// which the answer says; any other error stays here.
    fn subpath_failed<T>(&self, lookup: Lookup, error: MarsError) -> Answer<T> {
        let detail = match error {
            MarsError::SubpathMissing { .. } => SubpathDetail::Missing,
            MarsError::SubpathNotDirectory { .. } => SubpathDetail::NotADirectory,
            MarsError::SubpathTraversal { .. } => SubpathDetail::Traversal,
            other => return self.failed(lookup, other),
        };
        Answer::Failed(FailedFetch::Subpath(detail))
    }

    /// The content directory of a commit, fetched once.
    fn content_dir(&self, url: &str, commit: &str) -> Answer<PathBuf> {
        let key = (url.to_string(), commit.to_string());
        if let Some(answer) = self.content.borrow().get(&key) {
            return answer.clone();
        }
        let tag = self.tag_of.borrow().get(&key).cloned();
        let answer =
            match self
                .provider
                .fetch_content(&SourceUrl::from(url), commit, tag.as_deref())
            {
                Ok(dir) => Answer::Found(dir),
                Err(error @ MarsError::LockedCommitUnreachable { .. }) => {
                    self.errors
                        .borrow_mut()
                        .insert(Lookup::Content(key.0.clone(), key.1.clone()), error);
                    Answer::Missing
                }
                Err(error) => self.failed(Lookup::Content(key.0.clone(), key.1.clone()), error),
            };
        self.content.borrow_mut().insert(key, answer.clone());
        answer
    }

    /// The directory a checkout is in.
    pub(crate) fn checkout_dir(&self, origin: &Origin) -> PathBuf {
        match origin {
            Origin::Git { url, commit } => {
                match self.content.borrow().get(&(url.clone(), commit.clone())) {
                    Some(Answer::Found(dir)) => dir.clone(),
                    _ => PathBuf::new(),
                }
            }
            Origin::Path { dir } => PathBuf::from(dir),
        }
    }

    pub(crate) fn rooted_dir(&self, rooted: &Rooted) -> PathBuf {
        let checkout = self.checkout_dir(&rooted.checkout);
        match &rooted.subpath {
            Some(subpath) => checkout.join(subpath),
            None => checkout,
        }
    }

    fn disk_tree(&self, root: &Path, lookup: Lookup) -> Answer<Rc<dyn TreeSource>> {
        match DiskTreeSource::new(root) {
            Ok(tree) => {
                let tree = Rc::new(tree);
                self.disk.borrow_mut().push(Rc::clone(&tree));
                Answer::Found(tree)
            }
            Err(error) => self.failed(lookup, error),
        }
    }

    /// The error that failed `failure`, for a failure that came from an answer; `None` for one the
    /// core decided itself.
    pub(crate) fn error_for(&self, failure: &Failure) -> Option<MarsError> {
        let mut errors = self.errors.borrow_mut();
        match failure {
            Failure::Path { name, .. } => errors.remove(&Lookup::Path(name.clone())),
            Failure::Unavailable { need, .. } => {
                let read = self.read_error.borrow_mut().take();
                let stored = match need {
                    Need::Tags { url } => errors.remove(&Lookup::Tags(url.clone())),
                    Need::Ref { url, name } => {
                        errors.remove(&Lookup::Ref(url.clone(), name.clone()))
                    }
                    Need::Tree { url, commit, path } => errors
                        .remove(&Lookup::Tree(url.clone(), commit.clone(), path.clone()))
                        .or_else(|| errors.remove(&Lookup::Content(url.clone(), commit.clone()))),
                };
                read.or(stored)
            }
            _ => None,
        }
    }
}

impl Answers for CliAnswers<'_> {
    fn tags(&self, url: &str) -> Option<Answer<Vec<Tag>>> {
        if let Some(answer) = self.tags.borrow().get(url) {
            return Some(answer.clone());
        }
        let answer = match self.provider.list_tags(&SourceUrl::from(url)) {
            Ok(tags) => {
                let mut tag_of = self.tag_of.borrow_mut();
                for tag in &tags {
                    tag_of.insert((url.to_string(), tag.commit.clone()), tag.tag.clone());
                }
                Answer::Found(tags)
            }
            Err(error) => self.failed(Lookup::Tags(url.to_string()), error),
        };
        self.tags
            .borrow_mut()
            .insert(url.to_string(), answer.clone());
        Some(answer)
    }

    fn reference(&self, url: &str, name: &str) -> Option<Answer<String>> {
        let key = (url.to_string(), name.to_string());
        if let Some(answer) = self.refs.borrow().get(&key) {
            return Some(answer.clone());
        }
        let answer = match self.provider.resolve_ref(&SourceUrl::from(url), name) {
            Ok(commit) => Answer::Found(commit),
            Err(error) => self.failed(Lookup::Ref(key.0.clone(), key.1.clone()), error),
        };
        self.refs.borrow_mut().insert(key, answer.clone());
        Some(answer)
    }

    fn tree(
        &self,
        url: &str,
        commit: &str,
        path: Option<&str>,
    ) -> Option<Answer<Rc<dyn TreeSource>>> {
        let key = (
            url.to_string(),
            commit.to_string(),
            path.map(str::to_string),
        );
        if let Some(answer) = self.trees.borrow().get(&key) {
            return Some(answer.clone());
        }
        let lookup = Lookup::Tree(key.0.clone(), key.1.clone(), key.2.clone());
        let answer = match self.content_dir(url, commit) {
            Answer::Found(dir) => match path {
                None => self.disk_tree(&dir, lookup),
                Some(subpath) => {
                    // The package's own folder is the tree, so a file outside it is refused.
                    let subpath = SourceSubpath::new(subpath).expect("a subpath the core read");
                    match apply_subpath("", &dir, Some(&subpath)) {
                        Ok(root) => self.disk_tree(&root, lookup),
                        Err(error) => self.subpath_failed(lookup, error),
                    }
                }
            },
            Answer::Missing => Answer::Missing,
            Answer::Failed(reason) => Answer::Failed(reason),
        };
        self.trees.borrow_mut().insert(key, answer.clone());
        Some(answer)
    }

    fn path(&self, request: &PathRequest<'_>) -> Answer<PathSource> {
        let key = (
            request.name.to_string(),
            request.path.to_string(),
            request.subpath.map(str::to_string),
            request.declared_in.cloned(),
        );
        if let Some(answer) = self.paths.borrow().get(&key) {
            return answer.clone();
        }
        let lookup = Lookup::Path(request.name.to_string());
        let path = Path::new(request.path);
        let path = match request.declared_in {
            Some(declared_in) if path.is_relative() => self.rooted_dir(declared_in).join(path),
            _ => path.to_path_buf(),
        };
        let answer = match self.provider.fetch_path(&path, request.name) {
            Err(error) => self.failed(lookup, error),
            Ok(canonical) => {
                let root = match request.subpath {
                    Some(subpath) => {
                        let subpath = SourceSubpath::new(subpath).expect("a subpath the core read");
                        apply_subpath(request.name, &canonical, Some(&subpath))
                    }
                    None => Ok(canonical.clone()),
                };
                match root {
                    Err(error) => self.subpath_failed(lookup, error),
                    Ok(root) => match self.disk_tree(&root, lookup) {
                        Answer::Found(tree) => Answer::Found(PathSource {
                            canonical: canonical.to_string_lossy().into_owned(),
                            tree,
                        }),
                        Answer::Missing => Answer::Missing,
                        Answer::Failed(reason) => Answer::Failed(reason),
                    },
                }
            }
        };
        self.paths.borrow_mut().insert(key, answer.clone());
        answer
    }

    fn take_read_error(&self) -> Option<String> {
        let error = self
            .disk
            .borrow()
            .iter()
            .find_map(|tree| tree.take_error())?;
        let reason = error.to_string();
        *self.read_error.borrow_mut() = Some(error);
        Some(reason)
    }
}

/// The package folder below a checkout, which has to exist, be a directory and stay inside it.
pub(crate) fn apply_subpath(
    source_name: &str,
    checkout_root: &Path,
    subpath: Option<&SourceSubpath>,
) -> Result<PathBuf, MarsError> {
    let package_root = match subpath {
        Some(subpath) => {
            subpath
                .join_under(checkout_root)
                .map_err(|_| MarsError::SubpathTraversal {
                    source_name: source_name.to_string(),
                    subpath: subpath.to_string(),
                    checkout_root: checkout_root.to_path_buf(),
                })?
        }
        None => checkout_root.to_path_buf(),
    };

    if !package_root.exists() {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathMissing {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root does not exist under checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    if !package_root.is_dir() {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathNotDirectory {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root is not a directory under checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    let canonical_checkout = dunce::canonicalize(checkout_root).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!(
            "failed to canonicalize checkout root `{}`: {e}",
            checkout_root.display()
        ),
    })?;
    let canonical_package = dunce::canonicalize(&package_root).map_err(|e| MarsError::Source {
        source_name: source_name.to_string(),
        message: format!(
            "failed to canonicalize package root `{}`: {e}",
            package_root.display()
        ),
    })?;

    if !canonical_package.starts_with(&canonical_checkout) {
        return match subpath {
            Some(subpath) => Err(MarsError::SubpathTraversal {
                source_name: source_name.to_string(),
                subpath: subpath.to_string(),
                checkout_root: checkout_root.to_path_buf(),
            }),
            None => Err(MarsError::Source {
                source_name: source_name.to_string(),
                message: format!(
                    "package root escapes checkout root `{}`",
                    checkout_root.display()
                ),
            }),
        };
    }

    Ok(package_root)
}

/// The error `mars sync` raises for a failure the core found. A failure that came from an answer
/// returns that answer's own error.
pub(crate) fn mars_error(failure: Failure, answers: &CliAnswers<'_>) -> MarsError {
    if let Some(error) = answers.error_for(&failure) {
        return error;
    }
    let message = failure.message();
    match failure {
        Failure::Conflict { name, detail, .. } => ResolutionError::VersionConflict {
            name,
            message: detail,
        }
        .into(),
        Failure::Oscillation { name, steps } => {
            let detail = Failure::Oscillation {
                name: name.clone(),
                steps,
            }
            .conflict_detail()
            .expect("an oscillation has a detail");
            ResolutionError::VersionConflict {
                name,
                message: detail,
            }
            .into()
        }
        Failure::Identity(Identity::OneSourceTwoNames {
            existing,
            duplicate,
            source,
        }) => ResolutionError::DuplicateSourceIdentity {
            existing_name: existing,
            duplicate_name: duplicate,
            source_id: source,
        }
        .into(),
        Failure::Identity(Identity::OneNameTwoSources {
            name,
            existing,
            incoming,
        }) => ResolutionError::SourceIdentityMismatch {
            name,
            existing,
            incoming,
        }
        .into(),
        Failure::EngineIncompatible { name, message, .. } => {
            ResolutionError::RequiresEngineIncompatible { name, message }.into()
        }
        Failure::EngineUnsatisfiable { name, excluded } => {
            let detail = Failure::EngineUnsatisfiable {
                name: name.clone(),
                excluded,
            }
            .unsatisfiable_detail()
            .expect("an unsatisfiable failure has a detail");
            ResolutionError::RequiresMarsUnsatisfiable {
                name,
                message: detail,
            }
            .into()
        }
        Failure::EngineRequirementInvalid {
            name,
            engine,
            requirement,
            message,
        } => ResolutionError::InvalidEngineRequirement {
            package: name,
            engine,
            requirement,
            message,
        }
        .into(),
        Failure::EngineVersionInvalid {
            engine,
            version,
            message,
        } => ResolutionError::InvalidRunningEngineVersion {
            engine,
            version,
            message,
        }
        .into(),
        Failure::Plugin { text, .. } => MarsError::InvalidRequest { message: text },
        Failure::Frozen { message } => MarsError::FrozenViolation { message },
        Failure::Unreadable {
            rooted,
            diagnostics,
            ..
        } => ConfigError::Invalid {
            message: format!(
                "failed to parse {}: {}",
                answers.rooted_dir(&rooted).join("mars.toml").display(),
                diagnostics
                    .first()
                    .map_or("", |diagnostic| diagnostic.message.as_str())
            ),
        }
        .into(),
        Failure::Subpath {
            name,
            subpath,
            detail,
        } => MarsError::Source {
            source_name: name,
            message: format!("subpath `{subpath}` {}", detail.reason()),
        },
        // Answers that failed carry their own error above; a bound is the caller's to set.
        Failure::Path { .. } | Failure::Unavailable { .. } | Failure::Bounds { .. } => {
            MarsError::Internal(message)
        }
    }
}
