use super::*;
use mars_schema::dependency::Entry;

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
    let canonical_url = SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
        "https://example.com/org/shared.git",
    ));
    let mut deps = IndexMap::new();
    deps.insert(
        SourceName::from("dep-a"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(canonical_url.clone(), None),
            source_id: SourceId::git_with_subpath(canonical_url.clone(), None),
            spec: git_spec("git@example.com:org/shared.git", Some("v1.0.0")),
            subpath: None,
            filter: FilterMode::All,
            rename: RenameMap::new(),
            dialect: None,
        },
    );
    deps.insert(
        SourceName::from("dep-b"),
        EffectiveDependency {
            declared_source_id: SourceId::git_with_subpath(canonical_url.clone(), None),
            source_id: SourceId::git_with_subpath(canonical_url, None),
            spec: git_spec("https://example.com/org/shared.git", Some("v1.0.0")),
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

    let err = resolve(&config, &provider, None, &default_options())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("duplicate source identity"),
        "SSH and HTTPS of same repo should be detected as duplicate: {err}"
    );
}

/// TRUE mismatches (different host or path) still produce errors — canonicalization
/// does not collapse genuinely different repos into the same identity.
#[test]
fn different_host_or_path_does_not_produce_false_convergence() {
    // Different hosts → different SourceIds
    let github_id = SourceId::git_with_subpath(
        SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
            "https://github.com/org/repo.git",
        )),
        None,
    );
    let gitlab_id = SourceId::git_with_subpath(
        SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
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
        SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
            "https://github.com/org/repo-a.git",
        )),
        None,
    );
    let repo_b_id = SourceId::git_with_subpath(
        SourceUrl::from(mars_schema::git_url::canonicalize_git_url(
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
