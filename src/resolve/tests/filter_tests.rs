use super::*;
use mars_schema::item::ItemKind;

/// What `graph.selection` installs, as `(package, kind, slug)`.
fn selected(graph: &ResolvedGraph) -> Vec<(String, ItemKind, String)> {
    graph
        .selection
        .items
        .iter()
        .map(|item| (item.package.clone().unwrap(), item.kind, item.slug.clone()))
        .collect()
}

#[test]
fn direct_filter_selects_when_same_source_is_also_a_filtered_transitive_dep() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_dep = dir.path().join("dep");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_dep).unwrap();
    write_minimal_package_marker(&tree_dep);
    for skill in ["skill-a", "skill-b", "skill-c"] {
        write_skill(&tree_dep, skill);
    }

    let manifest_a = make_manifest_with_filters(
        "a",
        "1.0.0",
        vec![(
            "dep",
            "https://example.com/dep.git",
            ">=1.0.0",
            FilterConfig {
                skills: Some(vec!["skill-b".into(), "skill-c".into()]),
                ..FilterConfig::default()
            },
        )],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/dep.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("dep", tree_dep, None);

    let a_spec = git_spec("https://example.com/a.git", Some("v1.0.0"));
    let dep_spec = git_spec("https://example.com/dep.git", Some("v1.0.0"));
    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("a"),
        EffectiveDependency {
            declared_source_id: source_id_for_spec(&a_spec, None),
            source_id: source_id_for_spec(&a_spec, None),
            spec: a_spec,
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    dependencies.insert(
        SourceName::from("dep"),
        EffectiveDependency {
            declared_source_id: source_id_for_spec(&dep_spec, None),
            source_id: source_id_for_spec(&dep_spec, None),
            spec: dep_spec,
            subpath: None,
            filter: FilterMode::Include {
                agents: vec![],
                skills: vec!["skill-a".into(), "skill-b".into()],
            },
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    // `a`'s filtered entry for `dep` adds nothing to the consumer's own request.
    assert_eq!(
        selected(&graph),
        vec![
            ("dep".to_string(), ItemKind::Skill, "skill-a".to_string()),
            ("dep".to_string(), ItemKind::Skill, "skill-b".to_string()),
        ]
    );
}

#[test]
fn filtered_parent_dep_does_not_seed_unfiltered_grandchild_items() {
    let dir = TempDir::new().unwrap();
    let parent_tree = dir.path().join("parent");
    let child_tree = dir.path().join("child");
    let grandchild_tree = dir.path().join("grandchild");
    std::fs::create_dir_all(&parent_tree).unwrap();
    std::fs::create_dir_all(&child_tree).unwrap();
    std::fs::create_dir_all(&grandchild_tree).unwrap();
    write_minimal_package_marker(&parent_tree);
    write_minimal_package_marker(&child_tree);
    write_minimal_package_marker(&grandchild_tree);
    write_agent(&parent_tree, "runner", &[]);
    write_agent(&grandchild_tree, "danger", &["missing-skill"]);

    let parent_manifest = make_manifest(
        "parent",
        "1.0.0",
        vec![("child", "https://example.com/child.git", "v1.0.0")],
    );
    let child_manifest = make_manifest(
        "child",
        "1.0.0",
        vec![("grandchild", "https://example.com/grandchild.git", "v1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/parent.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/child.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/grandchild.git", vec![(1, 0, 0)]);
    provider.add_source("parent", parent_tree, Some(parent_manifest));
    provider.add_source("child", child_tree, Some(child_manifest));
    provider.add_source("grandchild", grandchild_tree, None);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("parent"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/parent.git"),
                None,
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/parent.git"),
                None,
            ),
            spec: git_spec("https://example.com/parent.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::Include {
                agents: vec!["runner".into()],
                skills: vec![],
            },
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(graph.nodes.contains_key("parent"));
    assert!(graph.nodes.contains_key("child"));
    assert!(graph.nodes.contains_key("grandchild"));
}

#[test]
fn filtered_parent_transitive_dep_materializes_only_frontmatter_required_items() {
    let dir = TempDir::new().unwrap();
    let parent_tree = dir.path().join("parent");
    let child_tree = dir.path().join("child");
    std::fs::create_dir_all(&parent_tree).unwrap();
    std::fs::create_dir_all(&child_tree).unwrap();
    write_minimal_package_marker(&parent_tree);
    write_minimal_package_marker(&child_tree);

    write_agent(&parent_tree, "runner", &["planning"]);
    write_agent(&child_tree, "child-agent", &[]);
    write_skill(&child_tree, "planning");
    write_skill(&child_tree, "unused");

    let parent_manifest = make_manifest(
        "parent",
        "1.0.0",
        vec![("child", "https://example.com/child.git", "v1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/parent.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/child.git", vec![(1, 0, 0)]);
    provider.add_source("parent", parent_tree, Some(parent_manifest));
    provider.add_source("child", child_tree, None);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("parent"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/parent.git"),
                None,
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/parent.git"),
                None,
            ),
            spec: git_spec("https://example.com/parent.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::Include {
                agents: vec!["runner".into()],
                skills: vec![],
            },
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    // The child contributes only the skill the selected Agent names.
    assert_eq!(
        selected(&graph),
        vec![
            ("parent".to_string(), ItemKind::Agent, "runner".to_string()),
            ("child".to_string(), ItemKind::Skill, "planning".to_string()),
        ]
    );

    let (target, renames, _, _) = crate::sync::target::build_with_collisions_and_diag(
        &graph,
        &config,
        &crate::types::SourceOrigin::LocalPackage.to_string().into(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    assert_eq!(target.items.len(), 2);
    assert!(target.items.contains_key("agents/runner.md"));
    assert!(target.items.contains_key("skills/planning"));
    assert!(!target.items.contains_key("agents/child-agent.md"));
    assert!(!target.items.contains_key("skills/unused"));
}
