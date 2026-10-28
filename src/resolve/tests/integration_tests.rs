use super::*;
use mars_schema::dependency::Entry;

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

    std::fs::write(dir.path().join("plugins"), "not a directory").unwrap();
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
fn transitive_override_does_not_collapse_conflicting_declared_identities() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let replacement = dir.path().join("replacement");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();

    let manifest_with_shared = |name: &str, relative_path: &str| Manifest {
        package: PackageInfo {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
        },
        dependencies: IndexMap::from([("shared".to_string(), Entry::at_path(relative_path))]),
        models: IndexMap::new(),
    };

    let mut provider = MockProvider::new();
    provider.add_source(
        "a",
        tree_a.clone(),
        Some(manifest_with_shared("a", "../shared-a")),
    );
    provider.add_source(
        "b",
        tree_b.clone(),
        Some(manifest_with_shared("b", "../shared-b")),
    );
    provider.add_source("shared", replacement.clone(), None);

    let config = make_config(vec![
        ("a", SourceSpec::Path(tree_a)),
        ("b", SourceSpec::Path(tree_b)),
    ]);
    let options = default_options()
        .with_source_overrides(IndexMap::from([(SourceName::from("shared"), replacement)]));

    let error = resolve(&config, &provider, None, &options)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("conflicting identities") && error.contains("shared"),
        "override must not erase conflicting declared provenance: {error}"
    );
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

    // Dep should be resolved to newest satisfying >=0.5.0 → 1.0.0
    let dep_node = &graph.nodes["dep"];
    assert_eq!(dep_node.resolved_ref.version, Some(Version::new(1, 0, 0)));

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
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/shared.git", Some("v1.0.0")),
            subpath: Some(subpath.clone()),
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    dependencies.insert(
        SourceName::from("b"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/shared.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/shared.git", Some("v1.0.0")),
            subpath: Some(subpath),
            filter: FilterMode::All,
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
        Entry {
            version: Some(">=1.0.0".to_string()),
            subpath: Some(SourceSubpath::new("plugins/bar").unwrap()),
            ..Entry::git("https://example.com/dep.git")
        },
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
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
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/a.git"),
                None,
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/a.git"),
                None,
            ),
            spec: git_spec("https://example.com/a.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    dependencies.insert(
        SourceName::from("dep"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/dep.git"),
                Some(SourceSubpath::new("plugins/foo").unwrap()),
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/dep.git"),
                Some(SourceSubpath::new("plugins/foo").unwrap()),
            ),
            spec: git_spec("https://example.com/dep.git", Some("v1.0.0")),
            subpath: Some(SourceSubpath::new("plugins/foo").unwrap()),
            filter: FilterMode::All,
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
    // A declared subpath flows into the dependency's identity and package root; a dependency
    // with none keeps `None`, and its package root is the checkout root.
    for subpath in [Some("plugins/foo"), None] {
        let dir = TempDir::new().unwrap();
        let tree_a = dir.path().join("a");
        let tree_dep = dir.path().join("dep");
        std::fs::create_dir_all(&tree_a).unwrap();
        std::fs::create_dir_all(tree_dep.join("plugins/foo")).unwrap();

        let subpath = subpath.map(|path| SourceSubpath::new(path).unwrap());
        let mut manifest_deps = IndexMap::new();
        manifest_deps.insert(
            "dep".to_string(),
            Entry {
                version: Some(">=1.0.0".to_string()),
                subpath: subpath.clone(),
                ..Entry::git("https://example.com/dep.git")
            },
        );
        let manifest_a = Manifest {
            package: PackageInfo {
                name: "a".to_string(),
                version: "1.0.0".to_string(),
                description: None,
                authors: None,
                homepage: None,
                license: None,
                schema: None,
                uses_providers: Vec::new(),
                requires_mars: None,
                requires_meridian: None,
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
            SourceId::git_with_subpath(SourceUrl::from("example.com/dep"), subpath.clone())
        );
        let expected_root = match &subpath {
            Some(subpath) => tree_dep.join(subpath.as_str()),
            None => tree_dep.clone(),
        };
        assert_eq!(dep_node.rooted_ref.package_root, expected_root);
        assert_eq!(dep_node.rooted_ref.checkout_root, tree_dep);
    }
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
        Entry::git("https://example.com/shared.git"),
    );
    let manifest_a = Manifest {
        package: PackageInfo {
            name: "a".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
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
            plugin: None,
            content: None,
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
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
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
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\nschema = 1\n\n[dependencies.shared]\npath = \"../shared\"\n",
    )
    .unwrap();
    std::fs::write(
        shared.join("mars.toml"),
        "[package]\nname = \"shared\"\nversion = \"1.0.0\"\nschema = 1\n\n[dependencies.planning]\npath = \"../planning\"\n",
    )
    .unwrap();
    std::fs::write(
        planning.join("mars.toml"),
        "[package]\nname = \"planning\"\nversion = \"1.0.0\"\nschema = 1\n",
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

// ========== RES-006 / RES-008: apply_subpath with None subpath ==========

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

    // The manifest sits in package_root, NOT the checkout root, so a read from
    // checkout_root finds none.
    let manifest = Manifest {
        package: PackageInfo {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
        },
        dependencies: IndexMap::new(),
        models: IndexMap::new(),
    };

    let subpath = SourceSubpath::new("plugins/foo").unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/repo.git", vec![(1, 0, 0)]);
    provider.trees.insert("dep".to_string(), checkout.clone());
    write_manifest(&package_root, &manifest);

    let mut dependencies = IndexMap::new();
    dependencies.insert(
        SourceName::from("dep"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/repo.git"),
                Some(subpath.clone()),
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from("https://example.com/repo.git"),
                Some(subpath.clone()),
            ),
            spec: git_spec("https://example.com/repo.git", Some("v1.0.0")),
            subpath: Some(subpath),
            filter: FilterMode::All,
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

// ========== RES-011: transitive dep with no subpath gets None identity ==========

// ========== URL Identity Convergence Tests ==========
//
// These tests verify that SSH and HTTPS URL forms of the same repository
// canonicalize to the same SourceId, preventing false-duplicate cache entries
// and enabling correct deduplication in the resolver.

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
    let ssh_canonical = SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
        "git@example.com:org/shared.git",
    ));
    let mut deps = IndexMap::new();
    deps.insert(
        SourceName::from("a"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(
                SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
                    "https://example.com/a.git",
                )),
                None,
            ),
            source_id: SourceId::git_with_subpath(
                SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
                    "https://example.com/a.git",
                )),
                None,
            ),
            spec: git_spec("https://example.com/a.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    deps.insert(
        SourceName::from("shared"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(ssh_canonical.clone(), None),
            source_id: SourceId::git_with_subpath(ssh_canonical, None),
            spec: git_spec("git@example.com:org/shared.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    let config = EffectiveConfig {
        dependencies: deps,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };

    // Resolution must succeed — SSH and HTTPS forms of the same repo converge.
    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(
        graph.nodes.contains_key("shared"),
        "shared should be resolved"
    );
    assert!(graph.nodes.contains_key("a"), "a should be resolved");
}

// ========== Re-resolution order-independence tests ==========
//
// When a transitive dep is resolved under a partial constraint set and a later
// intermediary adds a `Latest` constraint, the already-`Resolved` package must be
// re-resolved against the full accumulated constraints so the final version is
// order-independent.

/// Primary regression: `a` is processed first, then `b` adds a late `Latest`
/// constraint on `shared`. The final version must remain order-independent.
#[test]
fn transitive_latest_constraint_upgrades_already_resolved_version() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // `a` depends on `shared` with a semver bound.
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

    // Whichever of `a` (semver bound) or `b` (Latest) comes first in config, the result
    // must be 1.2.0.
    for order in [["a", "b"], ["b", "a"]] {
        let config = make_config(
            order
                .into_iter()
                .map(|name| {
                    (
                        name,
                        git_spec(&format!("https://example.com/{name}.git"), Some("v1.0.0")),
                    )
                })
                .collect(),
        );

        let graph = resolve(&config, &provider, None, &default_options()).unwrap();

        assert_eq!(graph.nodes.len(), 3, "a, b, shared should all be resolved");
        assert_eq!(
            graph.nodes["shared"].resolved_ref.version,
            Some(Version::new(1, 2, 0)),
            "shared must resolve to 1.2.0 (Latest+semver maximizes within >=1.0,<2.0), order {order:?}"
        );
    }
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
fn restart_fresh_context_drops_removed_transitive_engine_fallback() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared_v1 = dir.path().join("shared-v1");
    let tree_shared_v2 = dir.path().join("shared-v2");
    let tree_x_v1 = dir.path().join("x-v1");
    let tree_x_v2 = dir.path().join("x-v2");
    for tree in [
        &tree_a,
        &tree_b,
        &tree_shared_v1,
        &tree_shared_v2,
        &tree_x_v1,
        &tree_x_v2,
    ] {
        std::fs::create_dir_all(tree).unwrap();
    }

    // A resolves shared@v1 first; that version depends on X, whose newest version
    // is engine-incompatible. B then contributes Latest for shared, restarting at
    // shared@v2, which no longer depends on X.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![(
            "shared",
            "https://example.com/shared.git",
            ">=1.0.0, <3.0.0",
        )],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "")],
    );
    let manifest_shared_v1 = make_manifest(
        "shared",
        "1.0.0",
        vec![("x", "https://example.com/x.git", "")],
    );
    let manifest_shared_v2 = make_manifest("shared", "2.0.0", vec![]);

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versions("https://example.com/x.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_versioned_source("shared", "v1.0.0", tree_shared_v1, Some(manifest_shared_v1));
    provider.add_versioned_source("shared", "v2.0.0", tree_shared_v2, Some(manifest_shared_v2));
    provider.add_versioned_source(
        "x",
        "v1.0.0",
        tree_x_v1,
        Some(make_engine_manifest("x", "1.0.0", None, None)),
    );
    provider.add_versioned_source(
        "x",
        "v2.0.0",
        tree_x_v2,
        Some(make_engine_manifest("x", "2.0.0", Some(">=99"), None)),
    );

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: None,
            plugin: None,
            content: None,
        },
    );

    let (result, fallbacks) =
        resolve_with_report_details(&config, &provider, Some(&lock), &options);
    let graph = result.unwrap();
    assert!(!graph.nodes.contains_key("x"));
    assert!(
        fallbacks.is_empty(),
        "fallback report must not retain X from the abandoned resolution pass"
    );
}

#[test]
fn restart_fresh_context_selects_new_transitive_dependency_items() {
    let dir = TempDir::new().unwrap();
    let tree_a_v1 = dir.path().join("a-v1");
    let tree_a_v2 = dir.path().join("a-v2");
    let tree_b = dir.path().join("b");
    let tree_y = dir.path().join("y");
    std::fs::create_dir_all(&tree_a_v1).unwrap();
    std::fs::create_dir_all(&tree_a_v2).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_y).unwrap();
    write_minimal_package_marker(&tree_y);
    write_agent(&tree_y, "y-agent", &[]);

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

    assert!(
        graph
            .selection
            .items
            .iter()
            .any(|item| item.package.as_deref() == Some("y") && item.slug == "y-agent"),
        "Y is reached through unfiltered entries, so its items are installed"
    );
}

#[test]
fn latest_revisit_ignores_locked_commit_even_when_version_matches() {
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
            plugin: None,
            content: None,
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
        "`latest` must force current resolution instead of replaying the locked commit"
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
                declared_source_id: source_id_for_spec(&a_spec, None),
                source_id: source_id_for_spec(&a_spec, None),
                spec: a_spec,
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                dialect: None,
            },
        );

        let b_spec = git_spec(&b_url, Some("v1.0.0"));
        dependencies.insert(
            SourceName::from(b_name.clone()),
            EffectiveDependency {
                declared_source_id: source_id_for_spec(&b_spec, None),
                source_id: source_id_for_spec(&b_spec, None),
                spec: b_spec,
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                dialect: None,
            },
        );
    }

    let config = EffectiveConfig {
        dependencies,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
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

#[test]
fn engine_walk_down_selects_newest_compatible_version() {
    let dir = TempDir::new().unwrap();
    let old = dir.path().join("old");
    let new = dir.path().join("new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versioned_source(
        "a",
        "v1.0.0",
        old,
        Some(make_engine_manifest("a", "1.0.0", Some(">=0.1"), None)),
    );
    provider.add_versioned_source(
        "a",
        "v2.0.0",
        new,
        Some(make_engine_manifest("a", "2.0.0", Some(">=99"), None)),
    );
    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };

    let (result, diagnostics) = resolve_with_diagnostics(&config, &provider, None, &options);
    assert_eq!(
        result.unwrap().nodes["a"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "requires-mars-fallback" && d.message.contains("2.0.0"))
    );
}

#[test]
fn engine_walk_down_error_lists_every_rejected_candidate() {
    let dir = TempDir::new().unwrap();
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    for version in ["1.0.0", "2.0.0"] {
        let tree = dir.path().join(version);
        std::fs::create_dir_all(&tree).unwrap();
        provider.add_versioned_source(
            "a",
            &format!("v{version}"),
            tree,
            Some(make_engine_manifest("a", version, Some("<1"), None)),
        );
    }
    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    let error = resolve(&config, &provider, None, &options)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("1.0.0")
            && error.contains("2.0.0")
            && error.contains("use a mars version matching `<1`"),
        "{error}"
    );
}

#[test]
fn tightened_constraint_reports_persisted_engine_rejection() {
    let dir = TempDir::new().unwrap();
    let a_old = dir.path().join("a-old");
    let a_new = dir.path().join("a-new");
    let b_tree = dir.path().join("b");
    for tree in [&a_old, &a_new, &b_tree] {
        std::fs::create_dir_all(tree).unwrap();
    }

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versioned_source(
        "a",
        "v1.0.0",
        a_old,
        Some(make_engine_manifest("a", "1.0.0", None, None)),
    );
    provider.add_versioned_source(
        "a",
        "v2.0.0",
        a_new,
        Some(make_engine_manifest("a", "2.0.0", Some(">=99"), None)),
    );
    provider.add_source(
        "b",
        b_tree,
        Some(make_manifest(
            "b",
            "1.0.0",
            vec![("a", "https://example.com/a.git", ">=2")],
        )),
    );
    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some(">=1"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };

    let error = resolve(&config, &provider, None, &options).unwrap_err();
    match error {
        MarsError::Resolution(ResolutionError::RequiresMarsUnsatisfiable { name, message }) => {
            assert_eq!(name, "a");
            assert!(message.contains("2.0.0"), "{message}");
            assert!(message.contains("requires-mars `>=99`"), "{message}");
            assert!(message.contains("running 1.0.0"), "{message}");
        }
        other => panic!("expected persisted engine incompatibility, got {other:?}"),
    }
}

#[test]
fn frozen_rejects_engine_incompatible_lock() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_source(
        "a",
        tree,
        Some(make_engine_manifest("a", "1.0.0", Some(">=99"), None)),
    );
    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1")),
    )]);
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("locked".into()),
            plugin: None,
            content: None,
        },
    );
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..ResolveOptions::frozen()
    };
    assert!(matches!(
        resolve(&config, &provider, Some(&lock), &options),
        Err(MarsError::FrozenViolation { .. })
    ));
}

#[test]
fn incompatible_locked_version_warns_and_falls_back() {
    let dir = TempDir::new().unwrap();
    let locked_tree = dir.path().join("locked");
    let old_tree = dir.path().join("old");
    std::fs::create_dir_all(&locked_tree).unwrap();
    std::fs::create_dir_all(&old_tree).unwrap();
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source(
        "a",
        locked_tree,
        Some(make_engine_manifest("a", "2.0.0", Some(">=99"), None)),
    );
    provider.add_versioned_source(
        "a",
        "v1.0.0",
        old_tree,
        Some(make_engine_manifest("a", "1.0.0", None, None)),
    );
    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some(">=1")),
    )]);
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v2.0.0".into()),
            commit: Some("locked".into()),
            plugin: None,
            content: None,
        },
    );
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    let (result, diagnostics) = resolve_with_diagnostics(&config, &provider, Some(&lock), &options);
    assert_eq!(
        result.unwrap().nodes["a"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "requires-mars-lock-fallback")
    );
}

#[test]
fn refpin_and_path_engine_incompatibility_is_a_hard_error() {
    for use_path in [false, true] {
        let dir = TempDir::new().unwrap();
        let tree = dir.path().join("a");
        std::fs::create_dir_all(&tree).unwrap();
        let mut provider = MockProvider::new();
        provider.add_source(
            "a",
            tree.clone(),
            Some(make_engine_manifest("a", "1.0.0", Some(">=99"), None)),
        );
        let spec = if use_path {
            SourceSpec::Path(tree)
        } else {
            git_spec("https://example.com/a.git", Some("main"))
        };
        let config = make_config(vec![("a", spec)]);
        let options = ResolveOptions {
            mars_version: Some(Version::new(1, 0, 0)),
            ..default_options()
        };
        assert!(
            matches!(
                resolve(&config, &provider, None, &options),
                Err(MarsError::Resolution(
                    ResolutionError::RequiresEngineIncompatible { .. }
                ))
            ),
            "path source: {use_path}"
        );
    }
}

#[test]
fn either_engine_constraint_excludes_candidate_and_ignore_knobs_bypass_checks() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_source(
        "a",
        tree,
        Some(make_engine_manifest(
            "a",
            "1.0.0",
            Some(">=99"),
            Some(">=99"),
        )),
    );
    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);
    let enforced = ResolveOptions {
        mars_version: Some(Version::new(100, 0, 0)),
        meridian_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    assert!(matches!(
        resolve(&config, &provider, None, &enforced),
        Err(MarsError::Resolution(
            ResolutionError::RequiresMarsUnsatisfiable { .. }
        ))
    ));
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        meridian_version: Some(Version::new(1, 0, 0)),
        ignore_requires_mars: true,
        ignore_requires_meridian: true,
        ..default_options()
    };
    assert!(resolve(&config, &provider, None, &options).is_ok());
}

#[test]
fn transitive_engine_fallback_names_requiring_package() {
    let dir = TempDir::new().unwrap();
    let a_tree = dir.path().join("a");
    let b_old = dir.path().join("b-old");
    let b_new = dir.path().join("b-new");
    for tree in [&a_tree, &b_old, &b_new] {
        std::fs::create_dir_all(tree).unwrap();
    }
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source(
        "a",
        a_tree,
        Some(make_manifest(
            "a",
            "1.0.0",
            vec![("b", "https://example.com/b.git", ">=1")],
        )),
    );
    provider.add_versioned_source(
        "b",
        "v1.0.0",
        b_old,
        Some(make_engine_manifest("b", "1.0.0", None, None)),
    );
    provider.add_versioned_source(
        "b",
        "v2.0.0",
        b_new,
        Some(make_engine_manifest("b", "2.0.0", Some(">=99"), None)),
    );
    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    let (result, diagnostics) = resolve_with_diagnostics(&config, &provider, None, &options);
    assert_eq!(
        result.unwrap().nodes["b"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "requires-mars-fallback" && d.message.contains("required by a"))
    );
}

#[test]
fn engine_exclusions_survive_restart_and_prevent_oscillation() {
    let dir = TempDir::new().unwrap();
    let a_old = dir.path().join("a-old");
    let a_new = dir.path().join("a-new");
    let b_tree = dir.path().join("b");
    let c_tree = dir.path().join("c");
    let d_tree = dir.path().join("d");
    for tree in [&a_old, &a_new, &b_tree, &c_tree, &d_tree] {
        std::fs::create_dir_all(tree).unwrap();
    }
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/c.git", vec![(1, 0, 0), (1, 5, 0)]);
    provider.add_versions("https://example.com/d.git", vec![(1, 0, 0)]);
    let old_manifest = make_engine_manifest("a", "1.0.0", None, None);
    provider.add_source("a", a_old.clone(), Some(old_manifest.clone()));
    provider.add_versioned_source("a", "v1.0.0", a_old, Some(old_manifest));
    provider.add_versioned_source(
        "a",
        "v2.0.0",
        a_new,
        Some(make_engine_manifest("a", "2.0.0", Some(">=99"), None)),
    );
    provider.add_source(
        "b",
        b_tree,
        Some(make_manifest(
            "b",
            "1.0.0",
            vec![("a", "https://example.com/a.git", "")],
        )),
    );
    provider.add_source("c", c_tree, None);
    provider.add_source(
        "d",
        d_tree,
        Some(make_manifest(
            "d",
            "1.0.0",
            vec![("c", "https://example.com/c.git", "")],
        )),
    );
    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some(">=1"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
        ("c", git_spec("https://example.com/c.git", Some(">=1, <2"))),
        ("d", git_spec("https://example.com/d.git", Some("v1.0.0"))),
    ]);
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("locked-a".into()),
            plugin: None,
            content: None,
        },
    );
    lock.dependencies.insert(
        "c".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/c.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("locked-c".into()),
            plugin: None,
            content: None,
        },
    );
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };
    let (result, diagnostics) = resolve_with_diagnostics(&config, &provider, Some(&lock), &options);
    assert_eq!(
        result.unwrap().nodes["a"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.code == "requires-mars-fallback" && d.message.contains("`a` 2.0.0"))
            .count(),
        1,
        "a known-incompatible restart override must not be proposed again"
    );
}

#[test]
fn engine_fallback_report_tracks_final_selection_after_constraint_restart() {
    let dir = TempDir::new().unwrap();
    let a_v1 = dir.path().join("a-v1");
    let a_v2 = dir.path().join("a-v2");
    let a_v3 = dir.path().join("a-v3");
    let b_tree = dir.path().join("b");
    for tree in [&a_v1, &a_v2, &a_v3, &b_tree] {
        std::fs::create_dir_all(tree).unwrap();
    }

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (2, 0, 0), (3, 0, 0)],
    );
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versioned_source(
        "a",
        "v1.0.0",
        a_v1,
        Some(make_engine_manifest("a", "1.0.0", None, None)),
    );
    provider.add_versioned_source(
        "a",
        "v2.0.0",
        a_v2,
        Some(make_engine_manifest("a", "2.0.0", None, None)),
    );
    provider.add_versioned_source(
        "a",
        "v3.0.0",
        a_v3,
        Some(make_engine_manifest("a", "3.0.0", Some(">=99"), None)),
    );
    provider.add_source(
        "b",
        b_tree,
        Some(make_manifest(
            "b",
            "1.0.0",
            vec![("a", "https://example.com/a.git", "<2")],
        )),
    );

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some(">=1"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);
    let options = ResolveOptions {
        mars_version: Some(Version::new(1, 0, 0)),
        ..default_options()
    };

    let (result, fallbacks) = resolve_with_report_details(&config, &provider, None, &options);
    assert_eq!(
        result.unwrap().nodes["a"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(fallbacks[0].source, "a");
    assert_eq!(fallbacks[0].selected_version, "1.0.0");
    assert_eq!(fallbacks[0].skipped[0].version, "3.0.0");
}
