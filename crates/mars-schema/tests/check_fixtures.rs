use std::{collections::BTreeMap, fs};

use mars_schema::{
    check::check,
    dialect::Dialect,
    prepare::{PrepareContext, Prepared, prepare},
};
use serde::Deserialize;

mod common;

#[derive(Deserialize)]
struct Request {
    context: PrepareContext,
    #[serde(default)]
    declared_dialect: Option<Dialect>,
}

/// The codes `check` adds to what `prepare` returns for a path: the table in
/// `crates/mars-schema/src/check.rs`. The WASM runner holds the same list.
const CHECK_CODES: [&str; 14] = [
    "package.requirement",
    "agent.description-missing",
    "agent.name-missing",
    "definition.name-mismatch",
    "definition.duplicate-name",
    "definition.duplicate-slug",
    "selection.duplicate-slug",
    "skill-ref.missing",
    "skill-ref.provider",
    "skill-ref.external",
    "skill.no-definition",
    "field.restriction-lost",
    "field.execution-ignored",
    "frontmatter.repaired",
];

fn prepared(dir: &std::path::Path, context: PrepareContext, dialect: Option<Dialect>) -> Prepared {
    let tree = common::load_tree(dir);
    let mut prepared = prepare(&tree, context, dialect);
    mars_schema::links::package(&mut prepared);
    prepared
}

fn request(case: &std::path::Path) -> Request {
    serde_json::from_slice(&fs::read(case.join("request.json")).unwrap()).unwrap()
}

/// Every path's list, minus the check's own codes, is what `prepare` returns for it. It does not
/// depend on the graph, so no dependency is given.
fn assert_promise(case: &std::path::Path, prepared: &Prepared) {
    let tree = common::load_tree(&case.join("input"));
    let result = check(&tree, prepared, &BTreeMap::new());
    let text = |diagnostics: Vec<&mars_schema::diagnostic::SchemaDiagnostic>| {
        diagnostics
            .into_iter()
            .map(|diagnostic| serde_json::to_string(diagnostic).unwrap())
            .collect::<Vec<_>>()
    };
    let expected = prepared.all_diagnostics();
    for (path, found) in &result.diagnostics {
        let kept = found
            .iter()
            .filter(|diagnostic| !CHECK_CODES.contains(&diagnostic.code.as_str()))
            .collect();
        let mut want = expected.get(path.as_str()).cloned().unwrap_or_default();
        want.sort_by_key(|diagnostic| diagnostic.severity as u8);
        assert_eq!(text(kept), text(want), "{} {path}", case.display());
    }
    for path in expected.keys() {
        assert!(
            result.diagnostics.contains_key(*path),
            "{} {path}",
            case.display()
        );
    }
}

#[test]
fn check_fixtures_match() {
    for case in common::cases("check") {
        let request = request(&case);
        let root = prepared(
            &case.join("input"),
            request.context,
            request.declared_dialect,
        );
        let tree = common::load_tree(&case.join("input"));
        let mut dependencies = BTreeMap::new();
        let deps = case.join("deps");
        if deps.exists() {
            for dependency in common::cases_in(&deps) {
                let name = dependency.file_name().unwrap().to_str().unwrap().to_owned();
                dependencies.insert(
                    name,
                    prepared(&dependency, PrepareContext::Package, None).selection,
                );
            }
        }
        let result = check(&tree, &root, &dependencies);
        // Key order is part of the contract: the text is compared, not a parsed value.
        common::assert_file(&case.join("expected.json"), &common::pretty(&result));
    }
}

#[test]
fn check_never_disagrees_with_prepare() {
    for suite in ["prepare", "check"] {
        for case in common::cases(suite) {
            let request = request(&case);
            let root = prepared(
                &case.join("input"),
                request.context,
                request.declared_dialect,
            );
            assert_promise(&case, &root);
        }
    }
}
