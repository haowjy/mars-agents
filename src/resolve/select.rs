//! Builds the view of a settled graph that `mars_schema::selection::select` reads, and answers
//! for the parts only the binary knows: the bootstrap documents and hooks found in each
//! package's tree, and the `\` an exclude entry may spell on Windows.

use std::collections::HashMap;

use indexmap::IndexMap;
use mars_schema::filter::FilterMode;
use mars_schema::item::ItemKind;
use mars_schema::prepare::Prepared;
use mars_schema::selection::{GraphView, Item, PackageView, Request, SelectionResult, select};

use crate::discover::DiscoveredItem;
use crate::types::SourceName;

/// Select from the registry the resolver settled and the consumer's own entries.
pub(super) fn select_registry(
    config: &crate::config::EffectiveConfig,
    registry: &IndexMap<SourceName, super::RegisteredPackage>,
) -> SelectionResult {
    let packages = registry
        .iter()
        .map(|(name, package)| (name, &package.node, extra_items(package.items())))
        .collect();
    select_packages(consumer_requests(config), packages)
}

/// Selection for a hand-built graph, whose bootstrap documents and hooks are found on disk.
#[cfg(test)]
pub(crate) fn select_for_test(
    config: &crate::config::EffectiveConfig,
    nodes: &IndexMap<SourceName, super::ResolvedNode>,
) -> SelectionResult {
    let packages = nodes
        .iter()
        .map(|(name, node)| {
            let items = crate::discover::package_items(
                &node.rooted_ref.package_root,
                Some(name.as_str()),
                &node.prepared,
                None,
                &mut crate::diagnostic::DiagnosticCollector::new(),
            )
            .expect("test tree");
            (name, node, extra_items(items.iter()))
        })
        .collect();
    select_packages(consumer_requests(config), packages)
}

/// What installing the package being published would select, for `mars check`.
///
/// The package is the root: selected whole as a consumer's only request, with its `[dependencies]`
/// as its manifest entries, so its Agents' skill refs resolve as they will on a consumer's machine.
/// Bootstrap documents and hooks of the dependencies are not found: only the refs matter here.
pub(crate) fn select_published(
    prepared: &Prepared,
    graph: Option<&super::ResolvedGraph>,
) -> SelectionResult {
    select(&GraphView {
        requests: Vec::new(),
        root: Some(package_view(prepared, Vec::new())),
        packages: graph
            .iter()
            .flat_map(|graph| &graph.nodes)
            .map(|(name, node)| (name.to_string(), package_view(&node.prepared, Vec::new())))
            .collect(),
    })
}

fn select_packages(
    requests: Vec<Request>,
    packages: Vec<(&SourceName, &super::ResolvedNode, Vec<Item>)>,
) -> SelectionResult {
    let packages: HashMap<_, _> = packages
        .into_iter()
        .map(|(name, node, extras)| (name.to_string(), package_view(&node.prepared, extras)))
        .collect();
    select(&GraphView {
        requests,
        root: None,
        packages,
    })
}

fn consumer_requests(config: &crate::config::EffectiveConfig) -> Vec<Request> {
    config
        .dependencies
        .iter()
        .map(|(name, dependency)| Request {
            name: name.to_string(),
            filter: portable_filter(&dependency.filter),
        })
        .collect()
}

fn package_view(prepared: &Prepared, extra_items: Vec<Item>) -> PackageView {
    let mut view = PackageView::from_prepared(prepared, extra_items);
    for edge in &mut view.edges {
        edge.filter = portable_filter(&edge.filter);
    }
    view
}

/// The items preparation does not read: everything but Agents and skills.
fn extra_items<'a>(items: impl Iterator<Item = &'a DiscoveredItem>) -> Vec<Item> {
    items
        .filter(|item| !matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill))
        .map(|item| Item {
            kind: item.id.kind,
            slug: item.id.name.to_string(),
            path: slashed(&item.source_path.to_string_lossy()),
        })
        .collect()
}

/// Selection compares paths as written with `/`; on Windows a path-style exclude may spell it `\`.
fn portable_filter(filter: &FilterMode) -> FilterMode {
    match filter {
        FilterMode::Exclude(names) if cfg!(windows) => FilterMode::Exclude(
            names
                .iter()
                .map(|name| slashed(name.as_str()).into())
                .collect(),
        ),
        other => other.clone(),
    }
}

fn slashed(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}
