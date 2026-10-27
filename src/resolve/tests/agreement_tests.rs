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
    version: Option<Version>,
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
    let packages = HashMap::from([(
        SourceName::from("r"),
        SettledPackage {
            version: version.as_ref(),
            is_local,
        },
    )]);
    let mut diag = DiagnosticCollector::new();
    let result = check_selecting_requests_agree(selection, &constraints, &packages, &mut diag);
    (result, diag.drain())
}

#[test]
fn different_ref_pins_that_both_select_conflict() {
    let (result, _) = check(
        &selected_by(consumer_and_p()),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        None,
        false,
    );
    match result {
        Err(ResolutionError::PackageVersionConflict {
            package,
            existing,
            requested,
            chain,
        }) => {
            assert_eq!(package, "r");
            assert!(existing.contains("main") && existing.contains("mars.toml"));
            assert!(requested.contains("dev") && requested.contains("by p"));
            assert_eq!(chain, "p");
        }
        other => panic!("expected PackageVersionConflict, got {other:?}"),
    }
}

#[test]
fn a_request_that_selected_nothing_does_not_conflict() {
    let (result, diagnostics) = check(
        &selected_by(vec![Requester::Consumer]),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        None,
        false,
    );
    assert!(result.is_ok());
    assert!(diagnostics.is_empty());
}

#[test]
fn latest_beside_a_pin_that_missed_the_settled_version_warns() {
    let (result, diagnostics) = check(
        &selected_by(consumer_and_p()),
        vec![
            ("mars.toml", Constraint::Latest),
            ("p", Constraint::Semver("^1.0".parse().unwrap())),
        ],
        Some(Version::new(2, 0, 0)),
        false,
    );
    assert!(result.is_ok());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "potential-version-drift");
    assert!(
        diagnostics[0]
            .message
            .contains("package 'r' requested as ^1.0 by p but already seen as latest by mars.toml"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn latest_beside_a_pin_the_settled_version_meets_is_quiet() {
    let (result, diagnostics) = check(
        &selected_by(consumer_and_p()),
        vec![
            ("mars.toml", Constraint::Latest),
            ("p", Constraint::Semver("^1.0".parse().unwrap())),
        ],
        Some(Version::new(1, 4, 0)),
        false,
    );
    assert!(result.is_ok());
    assert!(diagnostics.is_empty());
}

#[test]
fn local_packages_never_conflict() {
    let (result, _) = check(
        &selected_by(consumer_and_p()),
        vec![("mars.toml", pin("main")), ("p", pin("dev"))],
        None,
        true,
    );
    assert!(result.is_ok());
}
