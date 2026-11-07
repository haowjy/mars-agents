use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use indexmap::IndexMap;

use crate::config::EffectiveConfig;
use crate::diagnostic::DiagnosticCollector;
use crate::resolve::ResolvedGraph;
use crate::types::SourceName;

use super::{ModelAlias, ResolvedDepModels};

pub(crate) fn declaration_ordered_dep_models(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
) -> Vec<ResolvedDepModels> {
    let mut decl_pos: HashMap<SourceName, usize> = HashMap::new();
    for (idx, name) in config.dependencies.keys().enumerate() {
        decl_pos.insert(name.clone(), idx);
    }

    // Propagate declaration position to transitives: a transitive dependency
    // takes the minimum position among all direct dependencies that reach it.
    for (idx, sponsor) in config.dependencies.keys().enumerate() {
        let Some(sponsor_node) = graph.nodes.get(sponsor) else {
            continue;
        };

        let mut queue: VecDeque<SourceName> = sponsor_node.deps().collect();
        let mut visited: HashSet<SourceName> = HashSet::new();

        while let Some(dep) = queue.pop_front() {
            if !visited.insert(dep.clone()) {
                continue;
            }

            decl_pos
                .entry(dep.clone())
                .and_modify(|pos| *pos = (*pos).min(idx))
                .or_insert(idx);

            if let Some(dep_node) = graph.nodes.get(&dep) {
                queue.extend(dep_node.deps());
            }
        }
    }

    // Build Kahn structures using dependency edges:
    // dep -> dependent (name depends on dep).
    let mut in_degree: HashMap<SourceName, usize> = HashMap::new();
    let mut adjacency: HashMap<SourceName, Vec<SourceName>> = HashMap::new();

    for name in graph.nodes.keys() {
        in_degree.entry(name.clone()).or_insert(0);
        adjacency.entry(name.clone()).or_default();
    }

    for (name, node) in &graph.nodes {
        for dep in node.deps() {
            if graph.nodes.contains_key(&dep) {
                *in_degree.entry(name.clone()).or_insert(0) += 1;
                adjacency.entry(dep.clone()).or_default().push(name.clone());
            }
        }
    }

    let mut ready: BinaryHeap<Reverse<(usize, SourceName)>> = BinaryHeap::new();
    for (name, degree) in &in_degree {
        if *degree == 0 {
            let position = decl_pos.get(name).copied().unwrap_or(usize::MAX);
            ready.push(Reverse((position, name.clone())));
        }
    }

    let mut ordered: Vec<SourceName> = Vec::with_capacity(graph.nodes.len());
    while let Some(Reverse((_, current))) = ready.pop() {
        ordered.push(current.clone());

        if let Some(dependents) = adjacency.get(&current) {
            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        let position = decl_pos.get(dependent).copied().unwrap_or(usize::MAX);
                        ready.push(Reverse((position, dependent.clone())));
                    }
                }
            }
        }
    }

    // Graph should already be acyclic from resolver; this keeps behavior
    // deterministic if that invariant is ever violated.
    let ordered_names: Vec<SourceName> = if ordered.len() == graph.nodes.len() {
        ordered
    } else {
        graph.order.clone()
    };

    ordered_names
        .iter()
        .filter_map(|name| {
            let node = graph.nodes.get(name)?;
            let manifest = node.manifest.as_ref()?;
            if manifest.models.is_empty() {
                return None;
            }
            Some(ResolvedDepModels {
                source_name: name.to_string(),
                models: manifest.models.clone(),
            })
        })
        .collect()
}

pub(crate) fn merged_model_aliases(
    graph: &ResolvedGraph,
    effective: &EffectiveConfig,
    diag: &mut DiagnosticCollector,
) -> IndexMap<String, ModelAlias> {
    let dep_models = declaration_ordered_dep_models(graph, effective);
    super::merge_model_config(&effective.models, &dep_models, diag, None)
}
