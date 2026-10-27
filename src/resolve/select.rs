//! Builds the view of a settled graph that `mars_schema::selection::select` reads, and answers
//! for the parts only the binary knows: the bootstrap documents and hooks found in each
//! package's tree, and the `\` an exclude entry may spell on Windows.

use indexmap::IndexMap;
use mars_schema::filter::FilterMode;
use mars_schema::item::ItemKind;
use mars_schema::selection::{GraphView, Item, PackageView, Request, Selection, select};

use crate::types::SourceName;

/// Select from the registry the resolver settled and the consumer's own entries.
pub(super) fn select_registry(
    config: &crate::config::EffectiveConfig,
    registry: &IndexMap<SourceName, super::RegisteredPackage>,
) -> Selection {
    select_nodes(
        config,
        registry.iter().map(|(name, package)| {
            let extras = package
                .items()
                .filter(|item| !matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill))
                .map(|item| Item {
                    kind: item.id.kind,
                    slug: item.id.name.to_string(),
                    path: slashed(&item.source_path.to_string_lossy()),
                })
                .collect();
            (name, &package.node, extras)
        }),
    )
}

/// Selection for a hand-built graph, whose bootstrap documents and hooks are found on disk.
#[cfg(test)]
pub(crate) fn select_for_test(
    config: &crate::config::EffectiveConfig,
    nodes: &IndexMap<SourceName, super::ResolvedNode>,
) -> Selection {
    select_nodes(
        config,
        nodes.iter().map(|(name, node)| {
            let prepared = node.prepared.as_deref().expect("a prepared test node");
            let extras = crate::discover::package_items(
                &node.rooted_ref.package_root,
                Some(name.as_str()),
                prepared,
                None,
                &mut crate::diagnostic::DiagnosticCollector::new(),
            )
            .expect("test tree")
            .into_iter()
            .filter(|item| !matches!(item.id.kind, ItemKind::Agent | ItemKind::Skill))
            .map(|item| Item {
                kind: item.id.kind,
                slug: item.id.name.to_string(),
                path: slashed(&item.source_path.to_string_lossy()),
            })
            .collect();
            (name, node, extras)
        }),
    )
}

fn select_nodes<'a>(
    config: &crate::config::EffectiveConfig,
    packages: impl Iterator<Item = (&'a SourceName, &'a super::ResolvedNode, Vec<Item>)>,
) -> Selection {
    let requests = config
        .dependencies
        .iter()
        .map(|(name, dependency)| Request {
            name: name.to_string(),
            filter: portable_filter(&dependency.filter),
        })
        .collect();
    let packages = packages
        .filter_map(|(name, node, extra_items)| {
            let view = PackageView {
                prepared: node.prepared.as_deref()?,
                edges: node
                    .edges
                    .iter()
                    .map(|edge| Request {
                        name: edge.name.to_string(),
                        filter: portable_filter(&edge.filter),
                    })
                    .collect(),
                uses_providers: node
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.package.uses_providers.clone())
                    .unwrap_or_default(),
                extra_items,
            };
            Some((name.to_string(), view))
        })
        .collect();
    select(&GraphView { requests, packages })
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
