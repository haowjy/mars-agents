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
    /// The package under `packages/` that is the root: selected whole, and not a dependency.
    root: Option<String>,
    /// A sibling case whose `packages/` this case uses.
    packages_from: Option<String>,
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
        let packages_dir = match &case.packages_from {
            Some(from) => case_dir.with_file_name(from).join("packages"),
            None => case_dir.join("packages"),
        };
        let mut names: Vec<String> = fs::read_dir(&packages_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        let prepared: BTreeMap<&str, Prepared> = names
            .iter()
            .map(|name| {
                let tree = common::load_tree(&packages_dir.join(name));
                (name.as_str(), prepare(&tree, PrepareContext::Package, None))
            })
            .collect();
        let package_view = |name: &String| {
            let extra = case.extra_items.get(name).cloned().unwrap_or_default();
            PackageView {
                facts: prepared[name.as_str()].selection.clone(),
                extra_items: extra,
            }
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
            root: case.root.as_ref().map(package_view),
            packages: names
                .iter()
                .filter(|name| case.root.as_ref() != Some(name))
                .map(|name| (name.clone(), package_view(name)))
                .collect(),
        };
        common::assert_file(
            &case_dir.join("expected.json"),
            &common::pretty(&select(&view)),
        );
    }
}
