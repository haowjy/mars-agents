//! The resolver over answers held in a map: the behaviors the CLI cannot reach, and the ones whose
//! failure would be a wrong version rather than an error.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::*;
use crate::tree::FileTree;

const D: &str = "https://example.com/";

#[derive(Default)]
struct Map {
    tags: BTreeMap<String, Vec<Tag>>,
    /// Each lookup of a ref takes the next commit, wrapping: a branch that moves.
    refs: RefCell<BTreeMap<String, (Vec<String>, usize)>>,
    trees: BTreeMap<(String, String), Rc<FileTree>>,
    paths: BTreeMap<String, Rc<FileTree>>,
    asked: RefCell<Vec<String>>,
}

impl Map {
    fn release(&mut self, repo: &str, version: &str, manifest: &str) {
        let url = format!("{D}{repo}.git");
        let commit = format!("{repo}-{version}");
        self.tags.entry(url.clone()).or_default().push(Tag {
            tag: format!("v{version}"),
            commit: commit.clone(),
        });
        let tree = FileTree::from_files([("mars.toml".to_string(), manifest.as_bytes().to_vec())])
            .unwrap();
        self.trees.insert((url, commit), Rc::new(tree));
    }
}

impl Answers for Map {
    fn tags(&self, url: &str) -> Option<Answer<Vec<Tag>>> {
        self.tags.get(url).cloned().map(Answer::Found)
    }

    fn reference(&self, url: &str, name: &str) -> Option<Answer<String>> {
        let mut refs = self.refs.borrow_mut();
        let (commits, next) = refs.get_mut(&format!("{url}#{name}"))?;
        let commit = commits[*next % commits.len()].clone();
        *next += 1;
        Some(Answer::Found(commit))
    }

    fn tree(
        &self,
        url: &str,
        commit: &str,
        _path: Option<&str>,
    ) -> Option<Answer<Rc<dyn TreeSource>>> {
        self.asked.borrow_mut().push(format!("{url}@{commit}"));
        let tree = self.trees.get(&(url.to_string(), commit.to_string()))?;
        Some(Answer::Found(Rc::clone(tree) as Rc<dyn TreeSource>))
    }

    fn path(&self, request: &PathRequest<'_>) -> Answer<PathSource> {
        match self.paths.get(request.path) {
            Some(tree) => Answer::Found(PathSource {
                canonical: request.path.to_string(),
                tree: Rc::clone(tree) as Rc<dyn TreeSource>,
            }),
            None => Answer::Failed(FailedFetch::Fetch("no such path".to_string())),
        }
    }
}

fn package(name: &str, extra: &str, deps: &[(&str, &str)]) -> String {
    let mut text =
        format!("[package]\nschema = 1\nname = \"{name}\"\nversion = \"1.0.0\"\n{extra}\n");
    for (dep, version) in deps {
        text.push_str(&format!(
            "\n[dependencies.{dep}]\nurl = \"{D}{dep}.git\"\nversion = \"{version}\"\n"
        ));
    }
    text
}

fn root(name: &str, version: &str) -> Dependency {
    Dependency {
        name: name.to_string(),
        declared: Spec::Git {
            url: format!("{D}{name}.git"),
            version: Some(version.to_string()),
        },
        spec: Spec::Git {
            url: format!("{D}{name}.git"),
            version: Some(version.to_string()),
        },
        subpath: None,
        plugin: None,
        constraint: crate::version::parse_constraint(Some(version)),
        required_by: "mars.toml".to_string(),
        declared_in: None,
        dialect: None,
    }
}

fn request(root: Vec<Dependency>) -> Request {
    Request {
        root,
        supplied: BTreeMap::new(),
        overrides: BTreeMap::new(),
        locked: BTreeMap::new(),
        mode: Mode::Sync,
        engine: Engine {
            mars: Version::new(1, 0, 0),
            meridian: None,
            ignore_requires_mars: false,
            ignore_requires_meridian: false,
        },
        bounds: Bounds::default(),
    }
}

fn resolved(outcome: Outcome) -> Resolved {
    match outcome {
        Outcome::Resolved(resolved) => resolved,
        Outcome::Needs(needs) => panic!("needs {needs:?}"),
        Outcome::Failed(failed) => panic!("failed: {}", failed.failure.message()),
    }
}

fn failed(outcome: Outcome) -> Failed {
    match outcome {
        Outcome::Failed(failed) => failed,
        Outcome::Resolved(_) => panic!("resolved"),
        Outcome::Needs(needs) => panic!("needs {needs:?}"),
    }
}

#[test]
fn engine_fallback_skips_every_newer_version_and_records_them() {
    let mut map = Map::default();
    map.release("d", "1.0.0", &package("d", "", &[]));
    for version in ["2.0.0", "3.0.0"] {
        map.release("d", version, &package("d", "requires-mars = \">=99\"", &[]));
    }
    let resolved = resolved(resolve(&request(vec![root("d", "")]), &map));
    assert_eq!(resolved.nodes["d"].resolved.tag.as_deref(), Some("v1.0.0"));
    let [fallback] = resolved.engine_fallbacks.as_slice() else {
        panic!("one fallback");
    };
    assert_eq!(fallback.selected_version, "1.0.0");
    let skipped: Vec<_> = fallback
        .skipped
        .iter()
        .map(|s| s.version.as_str())
        .collect();
    assert_eq!(skipped, ["3.0.0", "2.0.0"]);
    let advisories = resolved
        .diagnostics
        .iter()
        .filter(|d| matches!(d, Diagnostic::Note { code, .. } if code == "requires-mars-fallback"))
        .count();
    assert_eq!(advisories, 2, "one advisory per skipped version");
}

#[test]
fn engine_fallback_with_no_version_left_names_the_candidates() {
    let mut map = Map::default();
    map.release("d", "1.0.0", &package("d", "requires-mars = \">=99\"", &[]));
    let failure = failed(resolve(&request(vec![root("d", "")]), &map)).failure;
    assert!(
        matches!(&failure, Failure::EngineUnsatisfiable { excluded, .. } if excluded.len() == 1),
        "{}",
        failure.message()
    );
}

/// A branch that moves between lookups never settles: the restarts come back to a ref they chose.
#[test]
fn restarts_that_come_back_to_a_chosen_ref_are_an_oscillation() {
    let mut map = Map::default();
    let url = format!("{D}b.git");
    let tree = Rc::new(
        FileTree::from_files([("mars.toml".to_string(), package("b", "", &[]).into_bytes())])
            .unwrap(),
    );
    for commit in ["b1", "b2"] {
        map.trees
            .insert((url.clone(), commit.to_string()), Rc::clone(&tree));
    }
    map.refs
        .borrow_mut()
        .insert(format!("{url}#main"), (vec!["b1".into(), "b2".into()], 0));
    let outcome = resolve(&request(vec![root("b", "main"), root("b", "main")]), &map);
    let failure = failed(outcome).failure;
    let Failure::Oscillation { name, steps } = &failure else {
        panic!("{}", failure.message());
    };
    assert_eq!(name, "b");
    assert!(steps.len() >= 2, "{steps:?}");
    assert!(
        failure
            .message()
            .contains("resolution oscillation detected for `b`")
    );
}

#[test]
fn a_package_cycle_resolves_and_each_is_the_others_requester() {
    let mut map = Map::default();
    map.release("a", "1.0.0", &package("a", "", &[("b", "^1")]));
    map.release("b", "1.0.0", &package("b", "", &[("a", "^1")]));
    let resolved = resolved(resolve(&request(vec![root("a", "^1")]), &map));
    assert_eq!(resolved.nodes.keys().collect::<Vec<_>>(), ["a", "b"]);
    let requesters: Vec<_> = resolved.constraints["a"]
        .iter()
        .map(|(by, _)| by.as_str())
        .collect();
    assert_eq!(requesters, ["mars.toml", "b"]);
}

/// The edge that closes a cycle is a request like any other: it holds the package to its constraint.
#[test]
fn a_cycle_back_to_a_package_with_an_incompatible_constraint_conflicts() {
    let mut map = Map::default();
    map.release("a", "1.0.0", &package("a", "", &[("b", "^1")]));
    map.release("b", "1.0.0", &package("b", "", &[("a", "^2")]));
    let failure = failed(resolve(&request(vec![root("a", "^1")]), &map)).failure;
    assert!(
        matches!(&failure, Failure::Conflict { name, .. } if name == "a"),
        "{}",
        failure.message()
    );
}

/// A manifest whose dependency `b` is fetched from repository `repo`.
fn asks_b_from(repo: &str, constraint: &str) -> String {
    format!(
        "{}\n[dependencies.b]\nurl = \"{D}{repo}.git\"\nversion = \"{constraint}\"\n",
        package("a", "", &[])
    )
}

/// `a` 1.1.0 asks for `b` (its newest needs a newer Mars, so an older one is chosen and kept), then
/// `c` pins `a` to 1.0.0 and the walk restarts: the kept choice must not outlive what it answered.
fn restart_after_an_engine_fallback(a_1_0_0: &str, a_1_1_0: &str) -> Map {
    let mut map = Map::default();
    map.release("a", "1.0.0", a_1_0_0);
    map.release("a", "1.1.0", a_1_1_0);
    map.release("c", "1.0.0", &package("c", "", &[("a", "=1.0.0")]));
    map
}

#[test]
fn a_kept_choice_is_not_reused_for_another_source() {
    let mut map =
        restart_after_an_engine_fallback(&asks_b_from("newb", "^1"), &asks_b_from("oldb", "^1"));
    map.release("oldb", "1.0.0", &package("b", "", &[]));
    map.release(
        "oldb",
        "1.1.0",
        &package("b", "requires-mars = \">=99\"", &[]),
    );
    map.release("newb", "1.0.0", &package("b", "", &[]));
    let resolved = resolved(resolve(
        &request(vec![root("a", "^1"), root("c", "^1")]),
        &map,
    ));
    let b = &resolved.nodes["b"];
    assert_eq!(
        b.rooted.checkout,
        Origin::Git {
            url: format!("{D}newb.git"),
            commit: "newb-1.0.0".into()
        }
    );
}

#[test]
fn a_kept_choice_is_not_reused_against_a_new_constraint() {
    let mut map =
        restart_after_an_engine_fallback(&asks_b_from("b", "^2"), &asks_b_from("b", "^1"));
    map.release("b", "1.0.0", &package("b", "", &[]));
    map.release("b", "1.1.0", &package("b", "requires-mars = \">=99\"", &[]));
    map.release("b", "2.0.0", &package("b", "", &[]));
    let resolved = resolved(resolve(
        &request(vec![root("a", "^1"), root("c", "^1")]),
        &map,
    ));
    assert_eq!(resolved.nodes["b"].resolved.tag.as_deref(), Some("v2.0.0"));
}

#[test]
fn an_override_replaces_the_source_of_a_transitive_dependency() {
    let mut map = Map::default();
    map.release("a", "1.0.0", &package("a", "", &[("b", "^1")]));
    map.paths.insert(
        "/local/b".to_string(),
        Rc::new(
            FileTree::from_files([("mars.toml".to_string(), package("b", "", &[]).into_bytes())])
                .unwrap(),
        ),
    );
    let mut request = request(vec![root("a", "^1")]);
    request.overrides.insert("b".into(), "/local/b".into());
    let resolved = resolved(resolve(&request, &map));
    let b = &resolved.nodes["b"];
    assert_eq!(
        b.rooted.checkout,
        Origin::Path {
            dir: "/local/b".into()
        }
    );
    assert_eq!(b.fetch_url, None);
    assert!(
        !map.asked
            .borrow()
            .iter()
            .any(|asked| asked.contains("b.git")),
        "the overridden source is never fetched"
    );
}

/// A warning of a pass the walk discards is not repeated: the last pass reports once.
#[test]
fn a_restart_does_not_repeat_the_warnings_of_the_discarded_pass() {
    let mut map = Map::default();
    map.release("b", "1.0.0", &package("b", "", &[]));
    map.release("b", "1.1.0", &package("b", "", &[]));
    map.release("b", "1.2.0", &package("b", "", &[]));
    map.release("a", "1.0.0", &package("a", "", &[("b", "^1")]));
    map.release("c", "1.0.0", &package("c", "", &[("b", "=1.1.0")]));
    let mut request = request(vec![root("a", "^1"), root("c", "^1")]);
    // A lock row that names another repository for `b`: stale in every pass.
    request.locked.insert(
        "b".into(),
        LockedDependency {
            url: Some(format!("{D}other.git")),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("b-1.0.0".into()),
            plugin: None,
            content: None,
        },
    );
    // `a` takes b 1.2.0; `c` then asks for =1.1.0, which restarts the walk.
    let resolved = resolved(resolve(&request, &map));
    assert_eq!(resolved.nodes["b"].resolved.tag.as_deref(), Some("v1.1.0"));
    let stale = resolved
        .diagnostics
        .iter()
        .filter(
            |d| matches!(d, Diagnostic::Note { code, .. } if code == "stale-lock-source-identity"),
        )
        .count();
    assert_eq!(stale, 1);
}

#[test]
fn an_open_need_is_returned_and_a_failure_after_it_waits() {
    let mut map = Map::default();
    map.release("known", "1.0.0", &package("known", "", &[]));
    // `known` has no version ^2: a conflict, but only after the open need for `unknown`.
    let mut both = request(vec![root("unknown", "^1"), root("known", "^2")]);
    let Outcome::Needs(needs) = resolve(&both, &map) else {
        panic!("a failure after an open need waits");
    };
    assert_eq!(
        needs,
        vec![Need::Tags {
            url: format!("{D}unknown.git")
        }]
    );

    // The same work in the other order: the failure comes before any open need.
    both.root.reverse();
    let failure = failed(resolve(&both, &map)).failure;
    assert!(
        matches!(failure, Failure::Conflict { .. }),
        "{}",
        failure.message()
    );
}

#[test]
fn every_open_need_on_the_frontier_is_returned_together() {
    let map = Map::default();
    // Asked for b first; the needs come back sorted, not in walk order.
    let Outcome::Needs(needs) = resolve(&request(vec![root("b", "^1"), root("a", "^1")]), &map)
    else {
        panic!("needs");
    };
    let urls: Vec<_> = needs
        .iter()
        .map(|need| match need {
            Need::Tags { url } => url.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(urls, [format!("{D}a.git"), format!("{D}b.git")]);
}

#[test]
fn bytes_count_the_files_mars_reads_not_the_files_in_an_answer() {
    let mut map = Map::default();
    let manifest = package("a", "", &[]);
    let url = format!("{D}a.git");
    map.tags.insert(
        url.clone(),
        vec![Tag {
            tag: "v1.0.0".into(),
            commit: "a1".into(),
        }],
    );
    let tree = FileTree::from_files([
        ("mars.toml".to_string(), manifest.clone().into_bytes()),
        ("unread.bin".to_string(), vec![0; 100_000]),
    ])
    .unwrap();
    map.trees.insert((url, "a1".into()), Rc::new(tree));

    let mut within = request(vec![root("a", "^1")]);
    within.bounds.bytes = Some(10_000);
    resolved(resolve(&within, &map));

    let mut over = request(vec![root("a", "^1")]);
    over.bounds.bytes = Some(manifest.len() as u64 - 1);
    let failure = failed(resolve(&over, &map)).failure;
    assert!(
        matches!(
            &failure,
            Failure::Bounds { limit: BoundsLimit::Bytes, at } if at == "a"
        ),
        "{}",
        failure.message()
    );
}

#[test]
fn packages_are_bounded_by_the_closure() {
    let mut map = Map::default();
    map.release("a", "1.0.0", &package("a", "", &[("b", "^1")]));
    map.release("b", "1.0.0", &package("b", "", &[]));
    let mut bounded = request(vec![root("a", "^1")]);
    bounded.bounds.packages = Some(1);
    let failure = failed(resolve(&bounded, &map)).failure;
    // The limit is crossed by registering b, whatever is walked next.
    assert!(matches!(
        &failure,
        Failure::Bounds { limit: BoundsLimit::Packages, at } if at == "b"
    ));
}

#[test]
fn tags_that_are_not_versions_are_dropped_and_equal_versions_sort_by_tag() {
    let sorted = sort_tags(vec![
        Tag {
            tag: "v1.0.0".into(),
            commit: "x".into(),
        },
        Tag {
            tag: "nightly".into(),
            commit: "n".into(),
        },
        Tag {
            tag: "1.0.0".into(),
            commit: "y".into(),
        },
        Tag {
            tag: "v0.9.0".into(),
            commit: "z".into(),
        },
    ]);
    let tags: Vec<_> = sorted.iter().map(|tag| tag.tag.as_str()).collect();
    assert_eq!(tags, ["v0.9.0", "1.0.0", "v1.0.0"]);
}
