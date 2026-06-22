use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use indexmap::IndexMap;

use crate::config::{Config, EffectiveConfig, LocalConfig};
use crate::diagnostic::DiagnosticCollector;
use crate::resolve::ResolvedGraph;
use crate::types::SourceName;

use super::{ModelAlias, ResolvedDepModels};

pub(crate) fn declaration_ordered_dep_models(
    graph: &ResolvedGraph,
    config: &EffectiveConfig,
) -> Vec<ResolvedDepModels> {
    // Declaration positions for direct deps in consumer mars.toml.
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

        let mut queue: VecDeque<SourceName> = sponsor_node.deps.iter().cloned().collect();
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
                queue.extend(dep_node.deps.iter().cloned());
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
        for dep in &node.deps {
            if graph.nodes.contains_key(dep) {
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
    config: &Config,
    local: &LocalConfig,
    diag: &mut DiagnosticCollector,
) -> IndexMap<String, ModelAlias> {
    let consumer_models = crate::config::merged_models(&config.models, local);
    let dep_models = declaration_ordered_dep_models(graph, effective);
    super::merge_model_config(&consumer_models, &dep_models, diag, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EffectiveConfig, EffectiveDependency, FilterMode, Manifest, PackageInfo, Settings,
        SourceSpec,
    };
    use crate::resolve::{ResolvedGraph, ResolvedNode};
    use crate::types::SourceId;
    use std::path::PathBuf;

    fn model_alias(model: &str) -> ModelAlias {
        ModelAlias {
            harness: None,
            description: None,
            prompting: None,
            default_effort: None,
            autocompact: None,
            autocompact_pct: None,
            spec: super::super::ModelSpec::Pinned {
                model: model.to_string(),
                provider: None,
            },
        }
    }

    fn manifest_with_models(name: &str) -> Manifest {
        let mut models = IndexMap::new();
        models.insert(
            format!("{name}-alias"),
            model_alias(&format!("{name}-model")),
        );
        Manifest {
            package: PackageInfo {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                description: None,
            },
            dependencies: IndexMap::new(),
            models,
        }
    }

    fn resolved_node(name: &str, deps: &[&str], with_models: bool) -> ResolvedNode {
        let canonical = PathBuf::from(format!("/tmp/{name}"));
        ResolvedNode {
            source_name: name.into(),
            source_id: SourceId::Path {
                canonical: canonical.clone(),
                subpath: None,
            },
            rooted_ref: crate::resolve::RootedSourceRef {
                checkout_root: canonical.clone(),
                package_root: canonical.clone(),
            },
            resolved_ref: crate::source::ResolvedRef {
                source_name: name.into(),
                version: None,
                version_tag: None,
                commit: None,
                tree_path: canonical,
            },
            latest_version: None,
            manifest: with_models.then(|| manifest_with_models(name)),
            deps: deps.iter().map(|dep| (*dep).into()).collect(),
        }
    }

    fn effective_config_with_decl_order(names: &[&str]) -> EffectiveConfig {
        let mut dependencies = IndexMap::new();
        for name in names {
            let canonical = PathBuf::from(format!("/tmp/dep-{name}"));
            dependencies.insert(
                (*name).into(),
                EffectiveDependency {
                    name: (*name).into(),
                    id: SourceId::Path {
                        canonical: canonical.clone(),
                        subpath: None,
                    },
                    spec: SourceSpec::Path(canonical),
                    subpath: None,
                    filter: FilterMode::All,
                    rename: crate::types::RenameMap::new(),
                    dialect: None,
                    is_overridden: false,
                    original_git: None,
                },
            );
        }
        EffectiveConfig {
            dependencies,
            settings: Settings::default(),
            skills: indexmap::IndexMap::new(),
        }
    }

    fn dep_model_names(models: &[ResolvedDepModels]) -> Vec<String> {
        models.iter().map(|m| m.source_name.clone()).collect()
    }

    #[test]
    fn declaration_ordered_dep_models_sibling_order() {
        let mut nodes = IndexMap::new();
        nodes.insert("a".into(), resolved_node("a", &[], true));
        nodes.insert("b".into(), resolved_node("b", &[], true));

        let graph = ResolvedGraph {
            nodes,
            order: vec!["a".into(), "b".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        // Direct siblings follow consumer declaration order, not graph insertion order.
        let config = effective_config_with_decl_order(&["b", "a"]);

        let dep_models = declaration_ordered_dep_models(&graph, &config);
        assert_eq!(dep_model_names(&dep_models), vec!["b", "a"]);
    }

    #[test]
    fn declaration_ordered_dep_models_diamond_uses_minimum_sponsor_position() {
        let mut nodes = IndexMap::new();
        nodes.insert("a".into(), resolved_node("a", &["d"], true));
        nodes.insert("b".into(), resolved_node("b", &["d"], true));
        nodes.insert("d".into(), resolved_node("d", &[], true));

        let graph = ResolvedGraph {
            nodes,
            order: vec!["d".into(), "a".into(), "b".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let config = effective_config_with_decl_order(&["a", "b"]);

        let dep_models = declaration_ordered_dep_models(&graph, &config);
        assert_eq!(dep_model_names(&dep_models), vec!["d", "a", "b"]);
    }

    #[test]
    fn declaration_ordered_dep_models_transitives_follow_sponsor_declaration_order() {
        let mut nodes = IndexMap::new();
        nodes.insert("a".into(), resolved_node("a", &["d"], false));
        nodes.insert("b".into(), resolved_node("b", &["e"], false));
        nodes.insert("d".into(), resolved_node("d", &[], true));
        nodes.insert("e".into(), resolved_node("e", &[], true));

        let graph = ResolvedGraph {
            nodes,
            order: vec!["d".into(), "e".into(), "a".into(), "b".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let config = effective_config_with_decl_order(&["a", "b"]);

        let dep_models = declaration_ordered_dep_models(&graph, &config);
        assert_eq!(dep_model_names(&dep_models), vec!["d", "e"]);
    }

    #[test]
    fn declaration_ordered_dep_models_keeps_deps_before_dependents() {
        let mut nodes = IndexMap::new();
        nodes.insert("a".into(), resolved_node("a", &["d"], true));
        nodes.insert("d".into(), resolved_node("d", &[], true));

        let graph = ResolvedGraph {
            nodes,
            order: vec!["d".into(), "a".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        // D is declared after A, but topological ordering must still emit D first.
        let config = effective_config_with_decl_order(&["a", "d"]);

        let dep_models = declaration_ordered_dep_models(&graph, &config);
        assert_eq!(dep_model_names(&dep_models), vec!["d", "a"]);
    }

    #[test]
    fn declaration_ordered_dep_models_is_deterministic() {
        let mut nodes = IndexMap::new();
        nodes.insert("a".into(), resolved_node("a", &["d"], true));
        nodes.insert("b".into(), resolved_node("b", &["e"], true));
        nodes.insert("d".into(), resolved_node("d", &[], true));
        nodes.insert("e".into(), resolved_node("e", &[], true));

        let graph = ResolvedGraph {
            nodes,
            order: vec!["d".into(), "e".into(), "a".into(), "b".into()],
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        };
        let config = effective_config_with_decl_order(&["a", "b"]);

        let first = dep_model_names(&declaration_ordered_dep_models(&graph, &config));
        for _ in 0..10 {
            let current = dep_model_names(&declaration_ordered_dep_models(&graph, &config));
            assert_eq!(current, first);
        }
    }
}
