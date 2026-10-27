//! The requests that select items from one package must agree on its version.

use super::*;
use crate::resolve::version::{SettledPackage, check_selecting_requests_agree};
use mars_schema::item::ItemKind;
use mars_schema::selection::{Requester, SelectedItem, Selection};

fn pin(reference: &str) -> Constraint {
    Constraint::RefPin(reference.to_string())
}

/// A selection of one skill of `r`, required by each of `requesters`.
fn selected_by(requesters: Vec<Requester>) -> Selection {
    Selection {
        items: vec![SelectedItem {
            package: "r".into(),
            kind: ItemKind::Skill,
            slug: "planning".into(),
            path: "skills/planning".into(),
            required_by: requesters,
        }],
        ..Selection::default()
    }
}

fn consumer_and_p() -> Vec<Requester> {
    vec![
        Requester::Consumer,
        Requester::Package {
            package: "p".into(),
        },
    ]
}

fn check(
    selection: &Selection,
    constraints: Vec<(&str, Constraint)>,
    settled: (Option<Version>, Option<&str>),
    is_local: bool,
) -> (
    Result<(), ResolutionError>,
    Vec<crate::diagnostic::Diagnostic>,
) {
    let constraints = HashMap::from([(
        SourceName::from("r"),
        constraints
            .into_iter()
            .map(|(by, constraint)| (by.to_string(), constraint))
            .collect(),
    )]);
    let (version, tag) = settled;
    let packages = HashMap::from([(
        SourceName::from("r"),
        SettledPackage {
            version: version.as_ref(),
            tag,
            is_local,
        },
    )]);
    let mut diag = DiagnosticCollector::new();
    let result = check_selecting_requests_agree(selection, &constraints, &packages, &mut diag);
    (result, diag.drain())
}

fn settled_at_pin(reference: &str) -> (Option<Version>, Option<&str>) {
    (None, Some(reference))
}

#[test]
fn a_pin_that_differs_from_the_settled_ref_conflicts() {
    let (result, _) = check(
        &selected_by(consumer_and_p()),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        settled_at_pin("main"),
        false,
    );
    match result {
        Err(ResolutionError::PackageVersionConflict {
            package,
            existing,
            requested,
        }) => {
            assert_eq!(package, "r");
            assert_eq!(existing, "ref:main");
            assert_eq!(requested, "`p` requires ref:dev");
        }
        other => panic!("expected PackageVersionConflict, got {other:?}"),
    }
}

#[test]
fn the_pin_of_a_request_that_selected_nothing_decides_the_ref_and_conflicts() {
    // The consumer selects nothing from `r`; `p` selects all of it at `dev`, yet `r` settled at
    // the consumer's `main`.
    let (result, _) = check(
        &selected_by(vec![Requester::Package {
            package: "p".into(),
        }]),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        settled_at_pin("main"),
        false,
    );
    assert!(matches!(
        result,
        Err(ResolutionError::PackageVersionConflict { .. })
    ));
}

#[test]
fn an_agent_pulling_a_skill_counts_as_a_request_of_its_package() {
    let (result, _) = check(
        &selected_by(vec![
            Requester::Consumer,
            Requester::Item {
                package: "p".into(),
                kind: ItemKind::Agent,
                slug: "writer".into(),
            },
        ]),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        settled_at_pin("main"),
        false,
    );
    assert!(matches!(
        result,
        Err(ResolutionError::PackageVersionConflict { .. })
    ));
}

#[test]
fn a_request_that_selected_nothing_does_not_conflict() {
    let (result, diagnostics) = check(
        &selected_by(vec![Requester::Consumer]),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        settled_at_pin("main"),
        false,
    );
    assert!(result.is_ok());
    assert!(diagnostics.is_empty());
}

#[test]
fn latest_beside_a_pin_warns() {
    let (result, diagnostics) = check(
        &selected_by(consumer_and_p()),
        vec![("mars.toml", pin("main")), ("p", Constraint::Latest)],
        settled_at_pin("main"),
        false,
    );
    assert!(result.is_ok());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "potential-version-drift");
    assert!(
        diagnostics[0]
            .message
            .contains("package 'r' requested as latest by p but pinned to ref:main"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn local_packages_never_conflict() {
    let (result, _) = check(
        &selected_by(consumer_and_p()),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        settled_at_pin("main"),
        true,
    );
    assert!(result.is_ok());
}
