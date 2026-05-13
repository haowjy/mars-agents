use super::*;

#[test]
fn apply_subpath_success_case() {
    let dir = TempDir::new().unwrap();
    let package_root = dir.path().join("plugins/foo");
    std::fs::create_dir_all(&package_root).unwrap();

    let subpath = SourceSubpath::new("plugins/foo").unwrap();
    let rooted = apply_subpath(&SourceName::from("dep"), dir.path(), Some(&subpath)).unwrap();

    assert_eq!(rooted.checkout_root, dir.path());
    assert_eq!(rooted.package_root, package_root);
}

#[test]
fn apply_subpath_missing_directory_rejection() {
    let dir = TempDir::new().unwrap();
    let subpath = SourceSubpath::new("plugins/missing").unwrap();

    let err = apply_subpath(&SourceName::from("dep"), dir.path(), Some(&subpath))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist"),
        "missing directory should be rejected: {err}"
    );
}

#[test]
fn apply_subpath_file_not_dir_rejection() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("plugins");
    std::fs::write(&file_path, "not a directory").unwrap();
    let subpath = SourceSubpath::new("plugins").unwrap();

    let err = apply_subpath(&SourceName::from("dep"), dir.path(), Some(&subpath))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not a directory"),
        "file subpath should be rejected: {err}"
    );
}

#[cfg(unix)]
#[test]
fn apply_subpath_traversal_rejection() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let outside_pkg = outside.path().join("pkg");
    std::fs::create_dir_all(&outside_pkg).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    let subpath = SourceSubpath::new("escape").unwrap();

    let err = apply_subpath(&SourceName::from("dep"), dir.path(), Some(&subpath))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("escapes checkout root"),
        "symlink traversal should be rejected: {err}"
    );
}

// ========== Resolution tests ==========

#[test]
fn single_source_no_deps() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("source-a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (1, 1, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1.0")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    assert_eq!(graph.nodes.len(), 1);
    assert!(graph.nodes.contains_key("a"));
    assert_eq!(graph.order.len(), 1);
    assert_eq!(graph.order[0], "a");

    // MVS: should pick 1.0.0 (minimum)
    let node = &graph.nodes["a"];
    assert_eq!(node.resolved_ref.version, Some(Version::new(1, 0, 0)));
}

#[test]
fn two_sources_no_deps() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(2, 0, 0)]);
    provider.add_source("a", tree_a, None);
    provider.add_source("b", tree_b, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v2.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.order.len(), 2);
    // Both should be in the order (either order is valid since no deps)
    assert!(graph.order.contains(&"a".into()));
    assert!(graph.order.contains(&"b".into()));
}

#[test]
fn source_with_transitive_dep() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_dep = dir.path().join("dep");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_dep).unwrap();

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("dep", "https://example.com/dep.git", ">=0.5.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/dep.git",
        vec![(0, 4, 0), (0, 5, 0), (0, 6, 0), (1, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("dep", tree_dep, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    // Should have both 'a' and 'dep'
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.nodes.contains_key("a"));
    assert!(graph.nodes.contains_key("dep"));

    // Dep should be resolved to minimum satisfying >=0.5.0 → 0.5.0
    let dep_node = &graph.nodes["dep"];
    assert_eq!(dep_node.resolved_ref.version, Some(Version::new(0, 5, 0)));

    // Resolver output order is deterministic alphabetical.
    assert_eq!(graph.order, vec!["a", "dep"]);
}

#[test]
fn duplicate_source_identity_detects_same_url_and_subpath() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    std::fs::create_dir_all(tree_a.join("plugins/foo")).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, None);

    let subpath = SourceSubpath::new("plugins/foo").unwrap();
    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("a"),
        EffectiveDependency {
            name: "a".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/shared.git", Some("v1.0.0")),
            subpath: Some(subpath.clone()),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    dependencies.insert(
        SourceName::from("b"),
        EffectiveDependency {
            name: "b".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/shared.git", Some("v1.0.0")),
            subpath: Some(subpath),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
    };

    let err = resolve(&config, &provider, None, &default_options())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("duplicate source identity"),
        "expected duplicate identity error: {err}"
    );
}

#[test]
fn source_identity_mismatch_detects_different_subpaths_for_same_name() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_dep = dir.path().join("dep");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(tree_dep.join("plugins/foo")).unwrap();
    std::fs::create_dir_all(tree_dep.join("plugins/bar")).unwrap();

    let mut manifest_deps = IndexMap::new();
    manifest_deps.insert(
        "dep".to_string(),
        ManifestDep {
            url: Some(SourceUrl::from("https://example.com/dep.git")),
            path: None,
            subpath: Some(SourceSubpath::new("plugins/bar").unwrap()),
            version: Some(">=1.0.0".to_string()),
            filter: FilterConfig::default(),
        },
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            primary_agent: None,
            targets: None,
        },
        dependencies: manifest_deps,
        models: IndexMap::new(),
    };

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/dep.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("dep", tree_dep, None);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("a"),
        EffectiveDependency {
            name: "a".into(),
            id: SourceId::git(SourceUrl::from("https://example.com/a.git")),
            spec: git_spec("https://example.com/a.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    dependencies.insert(
        SourceName::from("dep"),
        EffectiveDependency {
            name: "dep".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/dep.git"),
                Some(SourceSubpath::new("plugins/foo").unwrap()),
            ),
            spec: git_spec("https://example.com/dep.git", Some("v1.0.0")),
            subpath: Some(SourceSubpath::new("plugins/foo").unwrap()),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
    };

    let err = resolve(&config, &provider, None, &default_options())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("conflicting identities"),
        "expected identity mismatch error: {err}"
    );
}

#[test]
fn transitive_dep_propagates_subpath_into_source_identity() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_dep = dir.path().join("dep");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(tree_dep.join("plugins/foo")).unwrap();

    let mut manifest_deps = IndexMap::new();
    manifest_deps.insert(
        "dep".to_string(),
        ManifestDep {
            url: Some(SourceUrl::from("https://example.com/dep.git")),
            path: None,
            subpath: Some(SourceSubpath::new("plugins/foo").unwrap()),
            version: Some(">=1.0.0".to_string()),
            filter: FilterConfig::default(),
        },
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            primary_agent: None,
            targets: None,
        },
        dependencies: manifest_deps,
        models: IndexMap::new(),
    };

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/dep.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("dep", tree_dep.clone(), None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);
    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    let dep_node = graph.nodes.get("dep").expect("dep should be resolved");
    // SourceId stores the canonical URL (no protocol, no .git suffix)
    assert_eq!(
        dep_node.source_id,
        SourceId::git_with_subpath(
            SourceUrl::from("example.com/dep"),
            Some(SourceSubpath::new("plugins/foo").unwrap())
        )
    );
    assert_eq!(
        dep_node.rooted_ref.package_root,
        tree_dep.join("plugins/foo")
    );
}

#[test]
fn compatible_constraints_from_two_dependents() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // Both a and b depend on shared with the same constraint.
    // The resolved version must satisfy both.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 2, 0), (1, 5, 0), (2, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    assert_eq!(graph.nodes.len(), 3);
    // MVS with >=1.0.0 from both → picks 1.0.0 (minimum satisfying all)
    let shared_node = &graph.nodes["shared"];
    assert_eq!(
        shared_node.resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
}

#[test]
fn narrower_second_constraint_upgrades_mvs_selection() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // a requires shared >=1.0.0, b requires shared >=1.5.0.
    // a is processed first: MVS picks 1.0.0.  Then b's >=1.5.0 arrives — 1.0.0 does
    // not satisfy it, so re-resolution selects min(>=1.0.0 ∩ >=1.5.0) = 1.5.0.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.5.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 2, 0), (1, 5, 0), (2, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options())
        .expect("both constraints are satisfiable; should resolve to 1.5.0");
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 5, 0)),
        "re-resolution must upgrade shared to the minimum satisfying both >=1.0.0 and >=1.5.0"
    );
}

#[test]
fn incompatible_constraints_produce_error() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // a requires shared >=2.0.0, b requires shared <1.0.0 — incompatible
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=2.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "<1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(0, 5, 0), (1, 0, 0), (2, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let result = resolve(&config, &provider, None, &default_options());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("shared"),
        "error should mention the conflicting source: {err}"
    );
}

#[test]
fn cycle_does_not_error() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    // a depends on b, b depends on a → cycle
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("b", "https://example.com/b.git", ">=1.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("a", "https://example.com/a.git", ">=1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert!(graph.nodes.contains_key("a"));
    assert!(graph.nodes.contains_key("b"));
}

#[test]
fn same_version_revisit_skips_and_package_fetches_once() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();
    write_minimal_package_marker(&tree_shared);
    write_skill(&tree_shared, "common");

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(graph.nodes.contains_key("shared"));
    assert_eq!(provider.fetch_count("shared"), 1);
}

#[test]
fn different_second_constraint_re_resolves_to_satisfying_version() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();
    write_minimal_package_marker(&tree_shared);
    write_skill(&tree_shared, "common");

    // a requires shared >=1.0.0 → MVS picks 1.0.0.
    // b requires shared >=2.0.0 — 1.0.0 doesn't satisfy it, so re-resolution runs.
    // Combined constraints: >=1.0.0 ∩ >=2.0.0 → MVS selects 2.0.0.  No conflict.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.0.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=2.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options())
        .expect(">=1.0.0 and >=2.0.0 are jointly satisfiable by 2.0.0; should not error");
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(2, 0, 0)),
        "re-resolution must upgrade shared to 2.0.0 (min satisfying >=1.0.0 ∩ >=2.0.0)"
    );
}

#[test]
fn latest_and_pinned_revisit_re_resolves_to_pinned_version() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();
    write_minimal_package_marker(&tree_shared);
    write_skill(&tree_shared, "common");

    let mut deps_a = IndexMap::new();
    deps_a.insert(
        "shared".to_string(),
        ManifestDep {
            url: Some(SourceUrl::from("https://example.com/shared.git")),
            path: None,
            subpath: None,
            version: None,
            filter: FilterConfig::default(),
        },
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            primary_agent: None,
            targets: None,
        },
        dependencies: deps_a,
        models: IndexMap::new(),
    };
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "v1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    // a (Latest) is processed first: maximize → 2.0.0.
    // b arrives with exact =1.0.0 — 2.0.0 does not satisfy it, so re-resolution runs.
    // Combined: Latest (maximize) ∩ =1.0.0 → only 1.0.0 satisfies, maximize picks it.
    let graph = resolve(&config, &provider, None, &default_options())
        .expect("Latest + exact-pin are jointly satisfiable by 1.0.0; should not error");
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 0, 0)),
        "re-resolution must downgrade shared from 2.0.0 to 1.0.0 to satisfy the exact pin"
    );
}

#[test]
fn normal_mode_falls_back_when_locked_commit_unreachable() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (1, 1, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1.0")),
    )]);

    let unreachable_commit = "missing-locked-sha";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some(unreachable_commit.into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 1, 0))
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some("mock-commit")
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(unreachable_commit.to_string()), None]
    );
}

#[test]
fn frozen_mode_errors_when_locked_commit_unreachable() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (1, 1, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1.0")),
    )]);

    let unreachable_commit = "missing-locked-sha";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some(unreachable_commit.into()),
            tree_hash: None,
        },
    );

    let options = ResolveOptions {
        frozen: true,
        ..default_options()
    };
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(
        result,
        Err(MarsError::LockedCommitUnreachable { .. })
    ));
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(unreachable_commit.to_string())]
    );
}

#[test]
fn source_without_manifest_has_no_transitive_deps() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree, None); // No manifest

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert_eq!(graph.nodes.len(), 1);
    assert!(graph.nodes["a"].deps.is_empty());
}

#[test]
fn path_source_resolves_without_version() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("local-source");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("local", tree.clone(), None);

    let config = make_config(vec![("local", SourceSpec::Path(tree))]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert_eq!(graph.nodes.len(), 1);
    let node = &graph.nodes["local"];
    assert!(node.resolved_ref.version.is_none());
    assert!(node.latest_version.is_none());
}

#[test]
fn local_path_source_resolves_transitive_path_dependencies() {
    let dir = TempDir::new().unwrap();
    let app = dir.path().join("app");
    let shared = dir.path().join("shared");
    let planning = dir.path().join("planning");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::create_dir_all(&planning).unwrap();

    std::fs::write(
        app.join("mars.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies.shared]\npath = \"../shared\"\n",
    )
    .unwrap();
    std::fs::write(
        shared.join("mars.toml"),
        "[package]\nname = \"shared\"\nversion = \"1.0.0\"\n\n[dependencies.planning]\npath = \"../planning\"\n",
    )
    .unwrap();
    std::fs::write(
        planning.join("mars.toml"),
        "[package]\nname = \"planning\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    write_agent(&app, "coder", &["planning"]);
    write_skill(&planning, "planning");

    let provider = MockProvider::new();
    let config = make_config(vec![("app", SourceSpec::Path(app))]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(graph.nodes.contains_key("app"));
    assert!(graph.nodes.contains_key("shared"));
    assert!(graph.nodes.contains_key("planning"));
}

// ========== Deterministic package order tests ==========

#[test]
fn alphabetical_order_linear_chain() {
    let mut nodes = IndexMap::new();
    nodes.insert(
        "c".into(),
        ResolvedNode {
            source_name: "c".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/c")),
            resolved_ref: dummy_ref("c"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["b".into()],
        },
    );
    nodes.insert(
        "b".into(),
        ResolvedNode {
            source_name: "b".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/b")),
            resolved_ref: dummy_ref("b"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["a".into()],
        },
    );
    nodes.insert(
        "a".into(),
        ResolvedNode {
            source_name: "a".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/a")),
            resolved_ref: dummy_ref("a"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec![],
        },
    );

    let order = alphabetical_order(&nodes);
    assert_eq!(order, vec!["a", "b", "c"]);
}

#[test]
fn alphabetical_order_ignores_dependency_shape() {
    // a depends on b and c, both depend on d
    let mut nodes = IndexMap::new();
    nodes.insert(
        "a".into(),
        ResolvedNode {
            source_name: "a".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/a")),
            resolved_ref: dummy_ref("a"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["b".into(), "c".into()],
        },
    );
    nodes.insert(
        "b".into(),
        ResolvedNode {
            source_name: "b".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/b")),
            resolved_ref: dummy_ref("b"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["d".into()],
        },
    );
    nodes.insert(
        "c".into(),
        ResolvedNode {
            source_name: "c".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/c")),
            resolved_ref: dummy_ref("c"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["d".into()],
        },
    );
    nodes.insert(
        "d".into(),
        ResolvedNode {
            source_name: "d".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/d")),
            resolved_ref: dummy_ref("d"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec![],
        },
    );

    let order = alphabetical_order(&nodes);
    assert_eq!(order, vec!["a", "b", "c", "d"]);
}

#[test]
fn alphabetical_order_no_deps() {
    let mut nodes = IndexMap::new();
    nodes.insert(
        "a".into(),
        ResolvedNode {
            source_name: "a".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/a")),
            resolved_ref: dummy_ref("a"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec![],
        },
    );
    nodes.insert(
        "b".into(),
        ResolvedNode {
            source_name: "b".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/b")),
            resolved_ref: dummy_ref("b"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec![],
        },
    );

    let order = alphabetical_order(&nodes);
    assert_eq!(order.len(), 2);
    // Deterministic alphabetical order for independent nodes
    assert_eq!(order, vec!["a", "b"]);
}

#[test]
fn alphabetical_order_is_stable_for_cycles() {
    let mut nodes = IndexMap::new();
    nodes.insert(
        "a".into(),
        ResolvedNode {
            source_name: "a".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/a")),
            resolved_ref: dummy_ref("a"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["b".into()],
        },
    );
    nodes.insert(
        "b".into(),
        ResolvedNode {
            source_name: "b".into(),
            source_id: SourceId::git(SourceUrl::from("example.com/b")),
            resolved_ref: dummy_ref("b"),
            rooted_ref: dummy_rooted_ref(),
            latest_version: None,
            manifest: None,
            deps: vec!["a".into()],
        },
    );

    let order = alphabetical_order(&nodes);
    assert_eq!(order, vec!["a", "b"]);
}

// ========== RES-006 / RES-008: apply_subpath with None subpath ==========

/// RES-006 / RES-008: When no subpath is specified, checkout_root IS the
/// package_root and the resolver produces a RootedSourceRef where both
/// fields point to the same directory.
#[test]
fn apply_subpath_none_yields_checkout_as_package_root() {
    let dir = TempDir::new().unwrap();
    let rooted = apply_subpath(&SourceName::from("dep"), dir.path(), None).unwrap();
    assert_eq!(rooted.checkout_root, dir.path());
    assert_eq!(rooted.package_root, dir.path());
}

// ========== RES-009: manifest reader is called with package_root ==========

/// RES-009: The resolver must pass `package_root` (not checkout_root) to
/// the manifest reader.  We arrange a subpath dep whose checkout_root has
/// no mars.toml but whose package_root (a subdirectory) does, then verify
/// that the manifest is successfully discovered — proving package_root was
/// used as the read base.
#[test]
fn resolver_reads_manifest_from_package_root_not_checkout_root() {
    let dir = TempDir::new().unwrap();
    let checkout = dir.path().join("checkout");
    let package_root = checkout.join("plugins/foo");
    std::fs::create_dir_all(&package_root).unwrap();

    // The manifest is associated with package_root, NOT the checkout root.
    // MockProvider keyed by tree_path: we register the manifest under
    // package_root so that a read from checkout_root would return None
    // while a read from package_root returns the manifest.
    let manifest = Manifest {
        package: PackageInfo {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            primary_agent: None,
            targets: None,
        },
        dependencies: IndexMap::new(),
        models: IndexMap::new(),
    };

    let subpath = SourceSubpath::new("plugins/foo").unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/repo.git", vec![(1, 0, 0)]);
    // Register tree at checkout but map manifest only for package_root
    provider.trees.insert("dep".to_string(), checkout.clone());
    provider
        .manifests
        .insert(package_root.clone(), Some(manifest.clone()));
    provider.manifests.insert(checkout.clone(), None);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("dep"),
        EffectiveDependency {
            name: "dep".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/repo.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/repo.git", Some("v1.0.0")),
            subpath: Some(subpath),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
    };

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let node = graph.nodes.get("dep").expect("dep should be in graph");
    // Manifest must be present — only possible if package_root was used
    assert!(
        node.manifest.is_some(),
        "manifest should be loaded from package_root; got None — checkout_root was likely used instead"
    );
    assert_eq!(node.rooted_ref.package_root, package_root);
    assert_eq!(node.rooted_ref.checkout_root, checkout);
}

// ========== RES-005: single fetch for same URL, multiple subpaths ==========

/// RES-005: Two dependencies at different subpaths of the same git URL
/// must not trigger a second fetch.  In our resolver the fetch is keyed by
/// (source name, URL) so two DISTINCT dep names pointing to the same URL
/// but different subpaths each call fetch_git_version once — but the test
/// verifies they both resolve successfully with distinct package_roots,
/// which is the observable contract from the resolver's perspective
/// (cache sharing is a source-layer concern; here we verify no error is
/// raised and both roots are distinct).
#[test]
fn two_subpaths_same_url_resolve_to_distinct_package_roots() {
    let dir = TempDir::new().unwrap();
    let checkout_a = dir.path().join("a");
    let checkout_b = dir.path().join("b");
    let pkg_a = checkout_a.join("plugins/foo");
    let pkg_b = checkout_b.join("plugins/bar");
    std::fs::create_dir_all(&pkg_a).unwrap();
    std::fs::create_dir_all(&pkg_b).unwrap();

    let subpath_foo = SourceSubpath::new("plugins/foo").unwrap();
    let subpath_bar = SourceSubpath::new("plugins/bar").unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/mono.git", vec![(1, 0, 0)]);
    provider.add_source("dep-a", checkout_a.clone(), None);
    provider.add_source("dep-b", checkout_b.clone(), None);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("dep-a"),
        EffectiveDependency {
            name: "dep-a".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/mono.git"),
                Some(subpath_foo.clone()),
            ),
            spec: git_spec("https://example.com/mono.git", Some("v1.0.0")),
            subpath: Some(subpath_foo),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    dependencies.insert(
        SourceName::from("dep-b"),
        EffectiveDependency {
            name: "dep-b".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/mono.git"),
                Some(subpath_bar.clone()),
            ),
            spec: git_spec("https://example.com/mono.git", Some("v1.0.0")),
            subpath: Some(subpath_bar),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
    };

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert_eq!(graph.nodes.len(), 2);

    let node_a = graph.nodes.get("dep-a").expect("dep-a should be resolved");
    let node_b = graph.nodes.get("dep-b").expect("dep-b should be resolved");
    // Each gets its own distinct package_root
    assert_eq!(node_a.rooted_ref.package_root, pkg_a);
    assert_eq!(node_b.rooted_ref.package_root, pkg_b);
    // checkout_roots differ because MockProvider returns different trees per name
    assert_ne!(
        node_a.rooted_ref.package_root,
        node_b.rooted_ref.package_root
    );
}

// ========== RES-011: transitive dep with no subpath gets None identity ==========

/// RES-011 contrast: a transitive dep whose manifest entry has NO subpath
/// should produce a source identity with subpath = None (not inherit from
/// the parent).
#[test]
fn transitive_dep_without_subpath_has_none_in_source_identity() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_dep = dir.path().join("dep");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_dep).unwrap();

    // 'a' depends on 'dep' with NO subpath declared
    let mut manifest_deps = IndexMap::new();
    manifest_deps.insert(
        "dep".to_string(),
        ManifestDep {
            url: Some(SourceUrl::from("https://example.com/dep.git")),
            path: None,
            subpath: None,
            version: Some(">=1.0.0".to_string()),
            filter: FilterConfig::default(),
        },
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            primary_agent: None,
            targets: None,
        },
        dependencies: manifest_deps,
        models: IndexMap::new(),
    };

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/dep.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("dep", tree_dep.clone(), None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);
    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    let dep_node = graph.nodes.get("dep").expect("dep should be in graph");
    // No subpath declared → identity must have subpath = None
    // SourceId stores the canonical URL (no protocol, no .git suffix)
    assert_eq!(
        dep_node.source_id,
        SourceId::git_with_subpath(SourceUrl::from("example.com/dep"), None)
    );
    // package_root equals checkout_root when subpath is None
    assert_eq!(dep_node.rooted_ref.package_root, tree_dep);
    assert_eq!(dep_node.rooted_ref.checkout_root, tree_dep);
}

// ========== URL Identity Convergence Tests ==========
//
// These tests verify that SSH and HTTPS URL forms of the same repository
// canonicalize to the same SourceId, preventing false-duplicate cache entries
// and enabling correct deduplication in the resolver.

/// SSH and HTTPS forms of the same repo produce equal canonical SourceIds.
#[test]
fn ssh_and_https_url_forms_have_same_canonical_source_id() {
    let ssh_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "git@example.com:org/repo.git",
        )),
        None,
    );
    let https_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "https://example.com/org/repo.git",
        )),
        None,
    );
    assert_eq!(
        ssh_id, https_id,
        "SSH and HTTPS of the same repo must produce equal SourceIds"
    );
}

/// Two direct deps with different names that both resolve to the same canonical URL
/// are detected as a duplicate-identity conflict.
#[test]
fn ssh_and_https_direct_deps_same_repo_detected_as_duplicate() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("shared");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    // Register versions for the SSH URL (used by spec of dep-a)
    provider.add_versions("git@example.com:org/shared.git", vec![(1, 0, 0)]);
    // Register versions for the HTTPS URL (used by spec of dep-b)
    provider.add_versions("https://example.com/org/shared.git", vec![(1, 0, 0)]);
    provider.add_source("dep-a", tree.clone(), None);
    provider.add_source("dep-b", tree, None);

    // Both deps canonicalize to the same SourceId
    let canonical_url = SourceUrl::from(crate::source::canonical::canonicalize_git_url(
        "https://example.com/org/shared.git",
    ));
    let mut deps = IndexMap::new();
    deps.insert(
        SourceName::from("dep-a"),
        EffectiveDependency {
            name: "dep-a".into(),
            id: SourceId::git_with_subpath(canonical_url.clone(), None),
            spec: git_spec("git@example.com:org/shared.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    deps.insert(
        SourceName::from("dep-b"),
        EffectiveDependency {
            name: "dep-b".into(),
            id: SourceId::git_with_subpath(canonical_url, None),
            spec: git_spec("https://example.com/org/shared.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies: deps,
        settings: Settings::default(),
    };

    let err = resolve(&config, &provider, None, &default_options())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("duplicate source identity"),
        "SSH and HTTPS of same repo should be detected as duplicate: {err}"
    );
}

/// A transitive dep declared with HTTPS form converges with a direct dep declared
/// with SSH form of the same repo — no SourceIdentityMismatch is raised.
#[test]
fn transitive_dep_https_converges_with_direct_dep_ssh_same_canonical() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // Manifest for "a" declares "shared" with HTTPS URL form
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/org/shared.git", ">=1.0.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    // SSH URL form — used by the direct dep's spec for version resolution
    provider.add_versions("git@example.com:org/shared.git", vec![(1, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);

    // Direct dep "shared" uses SSH URL, but stores the canonical SourceId.
    // SSH form: git@example.com:org/shared.git → canonical: example.com/org/shared
    // HTTPS form: https://example.com/org/shared.git → canonical: example.com/org/shared
    // Both are the same canonical, so the resolver should not raise SourceIdentityMismatch.
    let ssh_canonical = SourceUrl::from(crate::source::canonical::canonicalize_git_url(
        "git@example.com:org/shared.git",
    ));
    let mut deps = IndexMap::new();
    deps.insert(
        SourceName::from("a"),
        EffectiveDependency {
            name: "a".into(),
            id: SourceId::git_with_subpath(
                SourceUrl::from(crate::source::canonical::canonicalize_git_url(
                    "https://example.com/a.git",
                )),
                None,
            ),
            spec: git_spec("https://example.com/a.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    deps.insert(
        SourceName::from("shared"),
        EffectiveDependency {
            name: "shared".into(),
            id: SourceId::git_with_subpath(ssh_canonical, None),
            spec: git_spec("git@example.com:org/shared.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            is_overridden: false,
            original_git: None,
        },
    );
    let config = EffectiveConfig {
        dependencies: deps,
        settings: Settings::default(),
    };

    // Resolution must succeed — SSH and HTTPS forms of the same repo converge.
    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(
        graph.nodes.contains_key("shared"),
        "shared should be resolved"
    );
    assert!(graph.nodes.contains_key("a"), "a should be resolved");
}

/// TRUE mismatches (different host or path) still produce errors — canonicalization
/// does not collapse genuinely different repos into the same identity.
#[test]
fn different_host_or_path_does_not_produce_false_convergence() {
    // Different hosts → different SourceIds
    let github_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "https://github.com/org/repo.git",
        )),
        None,
    );
    let gitlab_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "https://gitlab.com/org/repo.git",
        )),
        None,
    );
    assert_ne!(
        github_id, gitlab_id,
        "Different hosts must produce distinct SourceIds"
    );

    // Different paths on the same host → different SourceIds
    let repo_a_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "https://github.com/org/repo-a.git",
        )),
        None,
    );
    let repo_b_id = SourceId::git_with_subpath(
        SourceUrl::from(crate::source::canonical::canonicalize_git_url(
            "https://github.com/org/repo-b.git",
        )),
        None,
    );
    assert_ne!(
        repo_a_id, repo_b_id,
        "Different repo paths must produce distinct SourceIds"
    );
}

// ========== Re-resolution order-independence tests ==========
//
// When a transitive dep is resolved under a partial constraint set and a later
// intermediary adds a `Latest` constraint, the already-`Resolved` package must be
// re-resolved against the full accumulated constraints so the final version is
// order-independent.

/// Primary regression: `a` is processed first, resolves `shared` to 1.0.0 (MVS
/// under `>=1.0.0, <2.0.0`).  Then `b` is processed and adds a `Latest` constraint
/// on `shared`.  The `Resolved` branch must detect the version shift and upgrade the
/// registry entry to 1.2.0.
#[test]
fn transitive_latest_constraint_upgrades_already_resolved_version() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // `a` depends on `shared` with a semver bound: MVS would pick 1.0.0.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![(
            "shared",
            "https://example.com/shared.git",
            ">=1.0.0, <2.0.0",
        )],
    );
    // `b` depends on `shared` with no version constraint → Latest → maximize.
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (1, 2, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    // `a` appears first in config — will be processed first, resolving `shared` to
    // 1.0.0 under MVS.  `b`'s `Latest` constraint must then trigger re-resolution
    // to 1.2.0.
    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    assert_eq!(graph.nodes.len(), 3, "a, b, shared should all be resolved");
    let shared_node = &graph.nodes["shared"];
    assert_eq!(
        shared_node.resolved_ref.version,
        Some(Version::new(1, 2, 0)),
        "shared must resolve to 1.2.0 (Latest+semver maximizes within >=1.0,<2.0)"
    );
}

/// Symmetric case: `b` is processed first (Latest → 1.2.0), then `a` adds
/// `>=1.0.0, <2.0.0`.  Since 1.2.0 already satisfies the semver bound, re-resolution
/// should keep `shared` at 1.2.0.  Verifies order-independence — the result must
/// match the primary regression test above.
#[test]
fn transitive_latest_constraint_order_independent_b_first() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![(
            "shared",
            "https://example.com/shared.git",
            ">=1.0.0, <2.0.0",
        )],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (1, 2, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    // `b` first: Latest selects 1.2.0.  `a` arrives with `>=1.0.0, <2.0.0` which
    // 1.2.0 satisfies — no version change, no re-resolution needed, stays at 1.2.0.
    let config = make_config(vec![
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();

    assert_eq!(graph.nodes.len(), 3, "a, b, shared should all be resolved");
    let shared_node = &graph.nodes["shared"];
    assert_eq!(
        shared_node.resolved_ref.version,
        Some(Version::new(1, 2, 0)),
        "shared must resolve to 1.2.0 regardless of processing order"
    );
}

#[test]
fn restart_fresh_context_drops_removed_transitive_dependency_and_lock_entry() {
    let dir = TempDir::new().unwrap();
    let tree_a_v1 = dir.path().join("a-v1");
    let tree_a_v2 = dir.path().join("a-v2");
    let tree_b = dir.path().join("b");
    let tree_x = dir.path().join("x");
    std::fs::create_dir_all(&tree_a_v1).unwrap();
    std::fs::create_dir_all(&tree_a_v2).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_x).unwrap();

    // A@v1 depends on X; A@v2 drops X.
    let manifest_a_v1 = make_manifest(
        "a",
        "1.0.0",
        vec![("x", "https://example.com/x.git", ">=1.0.0")],
    );
    let manifest_a_v2 = make_manifest("a", "2.0.0", vec![]);
    // B contributes a late Latest constraint on A to force restart to v2.
    let manifest_b = make_manifest("b", "1.0.0", vec![("a", "https://example.com/a.git", "")]);

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/x.git", vec![(1, 0, 0)]);
    provider.add_versioned_source("a", "v1.0.0", tree_a_v1, Some(manifest_a_v1));
    provider.add_versioned_source("a", "v2.0.0", tree_a_v2, Some(manifest_a_v2));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("x", tree_x, None);

    let config = make_config(vec![
        (
            "a",
            git_spec("https://example.com/a.git", Some(">=1.0.0, <3.0.0")),
        ),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(
        !graph.nodes.contains_key("x"),
        "X should be absent after fresh-context restart on A@v2"
    );

    let lock = crate::lock::build(
        &graph,
        &crate::sync::apply::ApplyResult {
            outcomes: Vec::new(),
        },
        &LockFile::empty(),
        std::collections::BTreeMap::new(),
    )
    .unwrap();
    assert!(
        !lock.dependencies.contains_key("x"),
        "lock dependencies should not keep removed transitive dep X"
    );
}

#[test]
fn restart_fresh_context_materializes_new_transitive_dependency_filters() {
    let dir = TempDir::new().unwrap();
    let tree_a_v1 = dir.path().join("a-v1");
    let tree_a_v2 = dir.path().join("a-v2");
    let tree_b = dir.path().join("b");
    let tree_y = dir.path().join("y");
    std::fs::create_dir_all(&tree_a_v1).unwrap();
    std::fs::create_dir_all(&tree_a_v2).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_y).unwrap();

    // A@v1 has no deps; A@v2 introduces Y.
    let manifest_a_v1 = make_manifest("a", "1.0.0", vec![]);
    let manifest_a_v2 = make_manifest(
        "a",
        "2.0.0",
        vec![("y", "https://example.com/y.git", ">=1.0.0")],
    );
    // Late Latest from B forces A to v2.
    let manifest_b = make_manifest("b", "1.0.0", vec![("a", "https://example.com/a.git", "")]);

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/y.git", vec![(1, 0, 0)]);
    provider.add_versioned_source("a", "v1.0.0", tree_a_v1, Some(manifest_a_v1));
    provider.add_versioned_source("a", "v2.0.0", tree_a_v2, Some(manifest_a_v2));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("y", tree_y, None);

    let config = make_config(vec![
        (
            "a",
            git_spec("https://example.com/a.git", Some(">=1.0.0, <3.0.0")),
        ),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(
        graph.nodes.contains_key("y"),
        "Y should be present after A re-resolves to v2"
    );

    let y_filters = graph
        .filters
        .get("y")
        .expect("Y must receive materialization filters on restart");
    assert!(
        y_filters
            .iter()
            .any(|filter| matches!(filter, FilterMode::All)),
        "Y should receive an unfiltered materialization request so sync includes it"
    );
}

#[test]
fn restart_replaces_locked_commit_when_latest_revisit_changes_ref_without_version_change() {
    let dir = TempDir::new().unwrap();
    let tree_shared = dir.path().join("shared");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_shared).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_source("shared", tree_shared, None);
    provider.add_source("b", tree_b, Some(manifest_b));

    let config = make_config(vec![
        (
            "shared",
            git_spec("https://example.com/shared.git", Some("^1.0")),
        ),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let locked_commit = "locked-sha-123";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some(locked_commit.into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert_eq!(
        graph.nodes["shared"].resolved_ref.commit.as_deref(),
        Some("mock-commit"),
        "restart should replace locked replay commit when Latest changes selected ref"
    );
}

#[test]
fn monotonic_restart_converges_for_more_than_32_packages() {
    let dir = TempDir::new().unwrap();
    let mut provider = MockProvider::new();
    let mut dependencies = IndexMap::new();
    const PAIR_COUNT: usize = 40;

    for idx in 0..PAIR_COUNT {
        let a_name = format!("a-{idx}");
        let b_name = format!("b-{idx}");
        let a_url = format!("https://example.com/{a_name}.git");
        let b_url = format!("https://example.com/{b_name}.git");
        let tree_a = dir.path().join(&a_name);
        let tree_b = dir.path().join(&b_name);
        std::fs::create_dir_all(&tree_a).unwrap();
        std::fs::create_dir_all(&tree_b).unwrap();

        let manifest_b = make_manifest(&b_name, "1.0.0", vec![(&a_name, &a_url, "")]);

        provider.add_versions(&a_url, vec![(1, 0, 0), (2, 0, 0)]);
        provider.add_versions(&b_url, vec![(1, 0, 0)]);
        provider.add_source(&a_name, tree_a, None);
        provider.add_source(&b_name, tree_b, Some(manifest_b));

        let a_spec = git_spec(&a_url, Some(">=1.0.0, <3.0.0"));
        dependencies.insert(
            SourceName::from(a_name.clone()),
            EffectiveDependency {
                name: a_name.clone().into(),
                id: source_id_for_spec(&a_spec, None),
                spec: a_spec,
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                is_overridden: false,
                original_git: None,
            },
        );

        let b_spec = git_spec(&b_url, Some("v1.0.0"));
        dependencies.insert(
            SourceName::from(b_name.clone()),
            EffectiveDependency {
                name: b_name.clone().into(),
                id: source_id_for_spec(&b_spec, None),
                spec: b_spec,
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                is_overridden: false,
                original_git: None,
            },
        );
    }

    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
    };

    let graph = resolve(&config, &provider, None, &default_options())
        .expect("monotonic one-restart-per-package convergence should succeed");

    assert_eq!(graph.nodes.len(), PAIR_COUNT * 2);
    for idx in 0..PAIR_COUNT {
        let a_name = format!("a-{idx}");
        assert_eq!(
            graph
                .nodes
                .get(a_name.as_str())
                .expect("each A package should resolve")
                .resolved_ref
                .version,
            Some(Version::new(2, 0, 0)),
            "{a_name} should converge to the latest satisfying version after restart"
        );
    }
}

#[test]
fn restart_override_preserves_latest_version_metadata() {
    let dir = TempDir::new().unwrap();
    let tree_shared = dir.path().join("shared");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_shared).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 2, 0), (2, 0, 0)],
    );
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_source("shared", tree_shared, None);
    provider.add_source("b", tree_b, Some(manifest_b));

    let config = make_config(vec![
        (
            "shared",
            git_spec("https://example.com/shared.git", Some(">=1.0.0, <2.0.0")),
        ),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let shared = graph.nodes.get("shared").expect("shared should resolve");
    assert_eq!(shared.resolved_ref.version, Some(Version::new(1, 2, 0)));
    assert_eq!(
        shared.latest_version,
        Some(Version::new(2, 0, 0)),
        "latest_version should survive override-based restart"
    );
    assert!(
        provider.fetch_count("shared") > 1,
        "shared should be re-resolved at least once to exercise override path"
    );
}

#[test]
fn oscillating_ref_selection_errors_with_ref_cycle() {
    let dir = TempDir::new().unwrap();
    let tree_shared = dir.path().join("shared");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_shared).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_source("shared", tree_shared, None);
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.set_commit_sequence("shared", vec!["osc-a", "osc-b"]);

    let config = make_config(vec![
        (
            "shared",
            git_spec("https://example.com/shared.git", Some("^1.0")),
        ),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let result = resolve(&config, &provider, None, &default_options());
    match result {
        Err(MarsError::Resolution(ResolutionError::VersionConflict { name, message })) => {
            assert_eq!(name, "shared");
            assert!(
                message.contains("resolution oscillation detected for `shared`"),
                "oscillation message should name package: {message}"
            );
            assert!(
                message.contains("v1.0.0@osc-a") || message.contains("v1.0.0@osc-b"),
                "oscillation message should include ref cycle details: {message}"
            );
        }
        other => panic!("expected oscillation VersionConflict, got {other:?}"),
    }
}
