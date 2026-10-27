use std::{collections::BTreeMap, fs};

use mars_schema::{
    filter::FilterConfig,
    prepare::{PrepareContext, Prepared, prepare},
    selection::{GraphView, Item, PackageView, Request, select},
};
use serde::Deserialize;

/// One case is a consumer's requests over the packages under `packages/`. Each package is
/// prepared like any other, and its manifest gives its edges and `uses-providers`.
#[derive(Deserialize)]
struct Case {
    requests: Vec<RequestSpec>,
    /// Registry order, when it is not the sorted package names.
    #[serde(default)]
    registry: Vec<String>,
    /// Bootstrap documents and hooks, which preparation does not read.
    #[serde(default)]
    extra_items: BTreeMap<String, Vec<Item>>,
}

#[derive(Deserialize)]
struct RequestSpec {
    name: String,
    #[serde(flatten)]
    filter: FilterConfig,
}

mod common;

#[test]
fn selection_fixtures_match() {
    for case_dir in common::cases("selection") {
        let case: Case =
            serde_json::from_slice(&fs::read(case_dir.join("request.json")).unwrap()).unwrap();
        let mut names: Vec<String> = fs::read_dir(case_dir.join("packages"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        let prepared: BTreeMap<&str, Prepared> = names
            .iter()
            .map(|name| {
                let tree = common::load_tree(&case_dir.join("packages").join(name));
                (name.as_str(), prepare(&tree, PrepareContext::Package, None))
            })
            .collect();
        let order = if case.registry.is_empty() {
            names.clone()
        } else {
            case.registry.clone()
        };
        let view = GraphView {
            requests: case
                .requests
                .iter()
                .map(|request| Request {
                    name: request.name.clone(),
                    filter: request.filter.to_mode(),
                })
                .collect(),
            packages: order
                .iter()
                .map(|name| {
                    let extra = case.extra_items.get(name).cloned().unwrap_or_default();
                    (
                        name.clone(),
                        PackageView::from_prepared(&prepared[name.as_str()], extra),
                    )
                })
                .collect(),
        };
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&select(&view)),
        );
    }
}
