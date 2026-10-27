//! Builds the view of a settled graph that `mars_schema::selection::select` reads, and answers
//! for the parts only the binary knows: the bootstrap documents and hooks found in each
//! package's tree, and the `\` an exclude entry may spell on Windows.

use indexmap::IndexMap;
use mars_schema::filter::FilterMode;
use mars_schema::item::ItemKind;
use mars_schema::prepare::Prepared;
use mars_schema::selection::{GraphView, Item, PackageView, Request, Selection, select};

use crate::discover::DiscoveredItem;
use crate::types::SourceName;

/// Select from the registry the resolver settled and the consumer's own entries.
pub(super) fn select_registry(
    config: &crate::config::EffectiveConfig,
    registry: &IndexMap<SourceName, super::RegisteredPackage>,
) -> Selection {
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
) -> Selection {
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

/// The name the package being published goes by in its own selection; it is no dependency's name.
pub(crate) const PUBLISHED_PACKAGE: &str = "(this package)";

/// What installing the package being published would select, for `mars check`.
///
/// The package is a consumer's only request, taken whole, and its `[dependencies]` are its manifest
/// entries, so its Agents' skill refs resolve as they will on a consumer's machine. Bootstrap
/// documents and hooks of the dependencies are not found: only the refs matter here.
pub(crate) fn select_published(
    prepared: &Prepared,
    effective: &crate::config::EffectiveConfig,
    uses_providers: Vec<String>,
    graph: Option<&super::ResolvedGraph>,
) -> Selection {
    let mut packages: IndexMap<String, PackageView> = IndexMap::new();
    packages.insert(
        PUBLISHED_PACKAGE.to_string(),
        PackageView {
            prepared,
            edges: consumer_requests(effective),
            uses_providers,
            extra_items: Vec::new(),
        },
    );
    for (name, node) in graph.iter().flat_map(|graph| &graph.nodes) {
        packages.insert(name.to_string(), node_view(node, Vec::new()));
    }
    select(&GraphView {
        requests: vec![Request {
            name: PUBLISHED_PACKAGE.to_string(),
            filter: FilterMode::All,
        }],
        packages,
    })
}

fn select_packages(
    requests: Vec<Request>,
    packages: Vec<(&SourceName, &super::ResolvedNode, Vec<Item>)>,
) -> Selection {
    let packages = packages
        .into_iter()
        .map(|(name, node, extras)| (name.to_string(), node_view(node, extras)))
        .collect();
    select(&GraphView { requests, packages })
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

fn node_view(node: &super::ResolvedNode, extra_items: Vec<Item>) -> PackageView<'_> {
    let mut view = PackageView::from_prepared(&node.prepared, extra_items);
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
