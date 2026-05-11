use super::*;

// ========== parse_version_constraint tests ==========

#[test]
fn parse_none_is_latest() {
    assert!(matches!(
        parse_version_constraint(None),
        VersionConstraint::Latest
    ));
}

#[test]
fn parse_empty_is_latest() {
    assert!(matches!(
        parse_version_constraint(Some("")),
        VersionConstraint::Latest
    ));
}

#[test]
fn parse_latest_string() {
    assert!(matches!(
        parse_version_constraint(Some("latest")),
        VersionConstraint::Latest
    ));
    assert!(matches!(
        parse_version_constraint(Some("LATEST")),
        VersionConstraint::Latest
    ));
}

#[test]
fn parse_exact_version() {
    match parse_version_constraint(Some("v1.2.3")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(1, 2, 3)));
            assert!(!req.matches(&Version::new(1, 2, 4)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_major_version() {
    match parse_version_constraint(Some("v2")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(2, 0, 0)));
            assert!(req.matches(&Version::new(2, 5, 3)));
            assert!(!req.matches(&Version::new(1, 9, 9)));
            assert!(!req.matches(&Version::new(3, 0, 0)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_major_minor_version() {
    match parse_version_constraint(Some("v2.1")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(2, 1, 0)));
            assert!(req.matches(&Version::new(2, 1, 5)));
            assert!(!req.matches(&Version::new(2, 0, 9)));
            assert!(!req.matches(&Version::new(2, 2, 0)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_semver_req_gte() {
    match parse_version_constraint(Some(">=0.5.0")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(0, 5, 0)));
            assert!(req.matches(&Version::new(1, 0, 0)));
            assert!(!req.matches(&Version::new(0, 4, 9)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_semver_req_caret() {
    match parse_version_constraint(Some("^2.0")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(2, 0, 0)));
            assert!(req.matches(&Version::new(2, 9, 0)));
            assert!(!req.matches(&Version::new(3, 0, 0)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_semver_req_tilde() {
    match parse_version_constraint(Some("~1.2")) {
        VersionConstraint::Semver(req) => {
            assert!(req.matches(&Version::new(1, 2, 0)));
            assert!(req.matches(&Version::new(1, 2, 9)));
            assert!(!req.matches(&Version::new(1, 3, 0)));
        }
        other => panic!("expected Semver, got {other:?}"),
    }
}

#[test]
fn parse_branch_ref() {
    match parse_version_constraint(Some("main")) {
        VersionConstraint::RefPin(ref_name) => {
            assert_eq!(ref_name, "main");
        }
        other => panic!("expected RefPin, got {other:?}"),
    }
}

#[test]
fn parse_commit_ref() {
    match parse_version_constraint(Some("abc123def456")) {
        VersionConstraint::RefPin(ref_name) => {
            assert_eq!(ref_name, "abc123def456");
        }
        other => panic!("expected RefPin, got {other:?}"),
    }
}

#[test]
fn locked_version_preferred_when_satisfies_constraint() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (1, 1, 0), (1, 2, 0)],
    );
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1.0")),
    )]);

    // Lock file says v1.1.0
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some("abc".into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // Should prefer locked version 1.1.0 over MVS minimum 1.0.0
    assert_eq!(node.resolved_ref.version, Some(Version::new(1, 1, 0)));
}

#[test]
fn locked_version_ignored_when_constraint_changed() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (2, 0, 0), (2, 1, 0)],
    );
    provider.add_source("a", tree, None);

    // Config now requires ^2.0
    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^2.0")),
    )]);

    // Lock file says v1.0.0 — no longer satisfies ^2.0
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("abc".into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // Locked version doesn't satisfy ^2.0, so MVS picks 2.0.0
    assert_eq!(node.resolved_ref.version, Some(Version::new(2, 0, 0)));
}

#[test]
fn locked_commit_is_used_when_reachable() {
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

    let locked_commit = "locked-sha-123";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some(locked_commit.into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
}

#[test]
fn maximize_mode_ignores_locked_commit() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (1, 1, 0), (1, 2, 0)],
    );
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
            version: Some("v1.0.0".into()),
            commit: Some(unreachable_commit.into()),
            tree_hash: None,
        },
    );

    let options = ResolveOptions {
        maximize: true,
        upgrade_targets: HashSet::new(),
        bump_direct_constraints: false,
        frozen: false,
    };
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 2, 0))
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
}

#[test]
fn latest_resolves_to_newest() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (2, 0, 0), (3, 0, 0)],
    );
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("latest")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // "latest" has no constraint, MVS picks minimum → 1.0.0
    // Actually, "latest" means any version. With MVS, minimum is 1.0.0.
    // But "latest" semantically means newest. Let me check the spec...
    // The spec says "@latest as any version (newest wins)"
    // So latest should pick the newest. Let me handle this in select_version.
    assert_eq!(node.resolved_ref.version, Some(Version::new(3, 0, 0)));
    assert_eq!(node.latest_version, Some(Version::new(3, 0, 0)));
}

#[test]
fn latest_constraint_does_not_skip_sibling_semver_validation() {
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
        vec![("shared", "https://example.com/shared.git", "latest")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
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

    let result = resolve(&config, &provider, None, &default_options());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("shared"),
        "error should mention the conflicting source: {err}"
    );
    assert!(
        err.contains("^1.0"),
        "error should mention the violated semver constraint: {err}"
    );
}

#[test]
fn equivalent_semver_syntax_accepts_same_resolved_version() {
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
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![(
            "shared",
            "https://example.com/shared.git",
            ">=1.0.0, <2.0.0",
        )],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 6, 0), (2, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options())
        .expect("equivalent semver syntax should not conflict");
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 0, 0))
    );
    assert_eq!(provider.fetch_count("shared"), 1);
}

#[test]
fn v2_resolves_to_major_range() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 9, 0), (2, 0, 0), (2, 1, 0), (2, 5, 0), (3, 0, 0)],
    );
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v2")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // v2 → >=2.0.0, <3.0.0, MVS picks minimum → 2.0.0
    assert_eq!(node.resolved_ref.version, Some(Version::new(2, 0, 0)));
}

#[test]
fn branch_ref_resolves_without_semver() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let node = &graph.nodes["a"];
    assert!(node.resolved_ref.version.is_none());
    assert!(node.latest_version.is_none());
    assert_eq!(node.resolved_ref.commit, Some("ref:main".into()));
}

#[test]
fn maximize_mode_picks_newest() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (1, 5, 0), (1, 9, 0)],
    );
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^1.0")),
    )]);

    let options = ResolveOptions {
        maximize: true,
        upgrade_targets: HashSet::new(),
        bump_direct_constraints: false,
        frozen: false,
    };

    let graph = resolve(&config, &provider, None, &options).unwrap();
    let node = &graph.nodes["a"];
    assert_eq!(node.resolved_ref.version, Some(Version::new(1, 9, 0)));
}

#[test]
fn maximize_with_specific_targets() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (1, 5, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(2, 0, 0), (2, 5, 0)]);
    provider.add_source("a", tree_a, None);
    provider.add_source("b", tree_b, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("^1.0"))),
        ("b", git_spec("https://example.com/b.git", Some("^2.0"))),
    ]);

    // Only upgrade "a", not "b"
    let options = ResolveOptions {
        maximize: true,
        upgrade_targets: HashSet::from(["a".into()]),
        bump_direct_constraints: false,
        frozen: false,
    };

    let graph = resolve(&config, &provider, None, &options).unwrap();
    // "a" should be maximized → 1.5.0
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 5, 0))
    );
    // "b" should use MVS → 2.0.0
    assert_eq!(
        graph.nodes["b"].resolved_ref.version,
        Some(Version::new(2, 0, 0))
    );
}

#[test]
fn bump_direct_constraints_ignores_direct_pin_for_target() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let options = ResolveOptions {
        maximize: true,
        upgrade_targets: HashSet::from([SourceName::from("a")]),
        bump_direct_constraints: true,
        frozen: false,
    };

    let graph = resolve(&config, &provider, None, &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(2, 0, 0))
    );
}

#[test]
fn no_available_versions_falls_back_to_head() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    // No versions registered → empty list
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    let node = &graph.nodes["a"];
    assert!(node.resolved_ref.version.is_none());
    assert_eq!(node.resolved_ref.commit, Some("ref:HEAD".into()));
}

#[test]
fn untagged_source_uses_locked_commit_when_available() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);

    let locked_commit = "locked-untagged-sha";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: None,
            commit: Some(locked_commit.into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
}

#[test]
fn untagged_source_falls_back_to_head_when_locked_commit_unreachable() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);

    let unreachable_commit = "missing-locked-sha";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: None,
            commit: Some(unreachable_commit.into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some("ref:HEAD")
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(unreachable_commit.to_string()), None]
    );
}

#[test]
fn frozen_mode_errors_for_untagged_locked_commit_unreachable() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec("https://example.com/a.git", None))]);

    let unreachable_commit = "missing-locked-sha";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: None,
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

// ========== EARS R1–R15: resolver lock semantics ==========

// R1: Direct dep with locked version satisfying constraint uses the locked version (lock replay).
// Relies on existing test `locked_version_preferred_when_satisfies_constraint` above.

// R2: Frozen + transitive dep with unreachable locked commit → hard error.
#[test]
fn r2_frozen_transitive_locked_commit_unreachable_errors() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );

    let unreachable = "frozen-missing-sha";
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (1, 1, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);
    provider.mark_unreachable_preferred_commit(unreachable);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("a-commit".into()),
            tree_hash: None,
        },
    );
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some(unreachable.into()),
            tree_hash: None,
        },
    );

    let options = ResolveOptions {
        frozen: true,
        ..default_options()
    };
    // With frozen, transitive deps also consult the lock → unreachable commit → error.
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(
        matches!(result, Err(MarsError::LockedCommitUnreachable { .. })),
        "frozen + unreachable transitive locked commit should error: {result:?}"
    );
}

// R3: Transitive deps ignore the consumer lock in normal sync; MVS selects the minimum.
#[test]
fn r3_transitive_dep_ignores_consumer_lock() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 1, 0), (1, 2, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    // Lock records shared@v1.2.0, but shared is transitive → lock must be ignored.
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("a-commit".into()),
            tree_hash: None,
        },
    );
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.2.0".into()),
            commit: Some("shared-locked-commit".into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    // Lock is ignored for transitive → MVS minimum satisfying ^1.0 → v1.0.0 (not v1.2.0).
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 0, 0)),
        "transitive lock version v1.2.0 must be ignored; MVS selects v1.0.0"
    );
    // The locked commit must not have been passed for `shared`.
    let commits = provider.seen_preferred_commits();
    let shared_preferred = commits.last().cloned().flatten();
    assert!(
        shared_preferred.is_none(),
        "transitive dep must not receive locked commit hint"
    );
}

// R4: Two intermediaries constrain the same transitive dep — constraints intersect;
//     MVS picks the minimum version satisfying both.
#[test]
fn r4_multi_intermediary_constraint_intersection_selects_minimum_satisfying_both() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // a requires shared >=1.1, b requires shared ^1.0.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", ">=1.1.0")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 1, 0), (1, 2, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    // Intersection of >=1.1.0 and ^1.0 is [1.1.0, 2.0.0). MVS picks 1.1.0.
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 1, 0)),
        "MVS must pick the minimum satisfying both constraints: 1.1.0"
    );
}

// R5: --frozen replays the full graph from lock, including transitive deps.
#[test]
fn r5_frozen_replays_transitive_from_lock() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );

    let locked_commit = "frozen-transitive-sha";
    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0), (1, 1, 0)]);
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("v1.0.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("a-commit".into()),
            tree_hash: None,
        },
    );
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some(locked_commit.into()),
            tree_hash: None,
        },
    );

    let options = ResolveOptions {
        frozen: true,
        ..default_options()
    };
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    // With frozen, transitive lock is respected → shared@v1.1.0 with locked commit.
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 1, 0)),
        "frozen must replay transitive lock version v1.1.0"
    );
    assert_eq!(
        graph.nodes["shared"].resolved_ref.commit.as_deref(),
        Some(locked_commit),
        "frozen must replay transitive locked commit"
    );
}

// R7: Direct dep without --frozen prefers locked version when constraint is satisfied.
// Covered by `locked_version_preferred_when_satisfies_constraint` above.

// R8: `mars upgrade <source>` maximizes only the named source; others use MVS.
// Covered by `maximize_with_specific_targets` above.

// R9: `mars upgrade` (no targets) maximizes all directs.
// Covered by `maximize_mode_picks_newest` above.

// R10: A source listed as both direct and transitive dep is resolved as direct (lock consulted).
#[test]
fn r10_direct_and_transitive_same_source_uses_lock() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // `a` transitively depends on `shared`. `shared` is also a direct dep.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^1.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 1, 0), (1, 2, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);

    // Both `a` and `shared` are direct deps.
    // `shared` is listed first so it is resolved as direct (with lock) before
    // `a`'s manifest would otherwise pull it in as a transitive dep.
    let config = make_config(vec![
        (
            "shared",
            git_spec("https://example.com/shared.git", Some("^1.0")),
        ),
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
    ]);

    // Lock says shared@v1.1.0.
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("a-commit".into()),
            tree_hash: None,
        },
    );
    lock.dependencies.insert(
        "shared".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/shared.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some("shared-direct-commit".into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    // `shared` is direct → lock consulted → v1.1.0 (not MVS v1.0.0).
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 1, 0)),
        "direct dep appearing as transitive too must still use locked version"
    );
}

// R11: Multi-intermediary empty constraint intersection → error naming constraints and sources.
// Covered by `latest_constraint_does_not_skip_sibling_semver_validation` above.

// R12: RefPin overrides semver version selection.
// Covered by `branch_ref_resolves_without_semver` above.

// R13: Multiple RefPins for the same source → first wins.
#[test]
fn r13_multiple_refpins_use_first() {
    // Two direct deps that both depend on `shared` via different RefPin constraints.
    // Because resolve_git_source short-circuits on first RefPin, the first encountered wins.
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
        vec![("shared", "https://example.com/shared.git", "main")],
    );
    let manifest_b = make_manifest(
        "b",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "dev")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions("https://example.com/b.git", vec![(1, 0, 0)]);
    // shared has no semver tags → goes through ref path
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("b", tree_b, Some(manifest_b));
    provider.add_source("shared", tree_shared, None);

    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        ("b", git_spec("https://example.com/b.git", Some("v1.0.0"))),
    ]);

    // Resolution should succeed (first RefPin wins); result has no semver version.
    let graph = resolve(&config, &provider, None, &default_options()).unwrap();
    assert!(
        graph.nodes.contains_key("shared"),
        "shared must be resolved"
    );
    assert!(
        graph.nodes["shared"].resolved_ref.version.is_none(),
        "RefPin sources have no semver version"
    );
}

// R14: Local path source — no version resolution, lock is irrelevant.
#[test]
fn r14_local_path_source_ignores_lock() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("local-pkg");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("local", tree.clone(), None);

    let config = make_config(vec![("local", SourceSpec::Path(tree.clone()))]);

    // Lock records a version for `local`, but path sources ignore it.
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "local".into(),
        crate::lock::LockedSource {
            url: None,
            path: Some(tree.to_string_lossy().into_owned()),
            subpath: None,
            version: Some("v9.9.9".into()),
            commit: Some("ignored-commit".into()),
            tree_hash: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert!(
        graph.nodes.contains_key("local"),
        "local path source must be resolved"
    );
    assert!(
        graph.nodes["local"].resolved_ref.version.is_none(),
        "path sources never have a semver version"
    );
}

// R15: Transitive constraint from a manifest validates against an already-resolved direct dep.
#[test]
fn r15_transitive_constraint_validates_against_resolved_direct() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // `a` requires shared ^2.0, but the direct dep pins shared to v1.x.
    let manifest_a = make_manifest(
        "a",
        "1.0.0",
        vec![("shared", "https://example.com/shared.git", "^2.0")],
    );

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a.git", vec![(1, 0, 0)]);
    provider.add_versions(
        "https://example.com/shared.git",
        vec![(1, 0, 0), (1, 5, 0), (2, 0, 0)],
    );
    provider.add_source("a", tree_a, Some(manifest_a));
    provider.add_source("shared", tree_shared, None);

    // Direct dep pins shared to ^1.0 (only v1.x satisfies this).
    let config = make_config(vec![
        ("a", git_spec("https://example.com/a.git", Some("v1.0.0"))),
        (
            "shared",
            git_spec("https://example.com/shared.git", Some("^1.0")),
        ),
    ]);

    // `a`'s manifest requires ^2.0 which conflicts with direct ^1.0 → resolution error.
    let result = resolve(&config, &provider, None, &default_options());
    assert!(
        result.is_err(),
        "conflicting transitive constraint must cause an error"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("shared"),
        "error must name the conflicting source: {err}"
    );
}
