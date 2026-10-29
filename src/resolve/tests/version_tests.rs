use super::*;

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
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // Should prefer locked version 1.1.0 over unlocked latest-compatible 1.2.0.
    assert_eq!(node.resolved_ref.version, Some(Version::new(1, 1, 0)));
    assert_eq!(node.resolved_ref.commit.as_deref(), Some("abc"));
    assert_eq!(provider.list_versions_count("https://example.com/a.git"), 0);
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

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^2.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("old-commit".into()),
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    let node = &graph.nodes["a"];
    // Locked version doesn't satisfy ^2.0, so latest-compatible picks 2.1.0.
    assert_eq!(node.resolved_ref.version, Some(Version::new(2, 1, 0)));
}

#[test]
fn stale_lock_entry_with_mismatched_url_is_ignored_in_normal_sync() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a-new.git", vec![(1, 0, 0), (1, 2, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a-new.git", Some("^1.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a-old.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("stale-lock-commit".into()),
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 2, 0)),
        "stale lock identity should be ignored; resolver should pick newest compatible"
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
}

#[test]
fn frozen_mode_errors_when_lock_entry_identity_url_mismatches_source() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions("https://example.com/a-new.git", vec![(1, 0, 0), (1, 2, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a-new.git", Some("^1.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a-old.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("stale-lock-commit".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
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
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::upgrade(HashSet::new(), false);
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 2, 0))
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
}

#[test]
fn latest_ignores_compatible_lock_and_resolves_to_newest() {
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

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v2.0.0".into()),
            commit: Some("locked-v2".into()),
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(3, 0, 0)),
        "`latest` must force newest resolution instead of replaying a compatible lock"
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
}

#[test]
fn latest_and_semver_constraints_re_resolve_to_intersection() {
    let dir = TempDir::new().unwrap();
    let tree_a = dir.path().join("a");
    let tree_b = dir.path().join("b");
    let tree_shared = dir.path().join("shared");
    std::fs::create_dir_all(&tree_a).unwrap();
    std::fs::create_dir_all(&tree_b).unwrap();
    std::fs::create_dir_all(&tree_shared).unwrap();

    // a uses "latest" for shared → maximize → 2.0.0 (available: 1.0.0, 2.0.0).
    // b uses "^1.0" (= >=1.0.0, <2.0.0) → 2.0.0 doesn't satisfy <2.0.0, triggering
    // re-resolution.  Combined: Latest (maximize) ∩ ^1.0 → satisfying = [1.0.0] →
    // maximize picks 1.0.0.  Both constraints are jointly satisfiable.
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

    let graph = resolve(&config, &provider, None, &default_options())
        .expect("latest + ^1.0 are jointly satisfiable by 1.0.0; should not error");
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 0, 0)),
        "re-resolution must select 1.0.0 (max version satisfying both latest-maximize and ^1.0)"
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
        Some(Version::new(1, 6, 0))
    );
    assert_eq!(provider.fetch_count("shared"), 1);
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
    assert_eq!(node.resolved_ref.version_tag.as_deref(), Some("main"));
    assert_eq!(node.resolved_ref.commit, Some("ref:main".into()));
}

#[test]
fn ref_pin_prefers_locked_commit_in_normal_sync() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let locked_commit = "locked-refpin-sha";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: Some(locked_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.version_tag.as_deref(),
        Some("main")
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
}

#[test]
fn ref_pin_falls_back_when_locked_commit_unreachable_in_normal_sync() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let unreachable_commit = "missing-refpin-sha";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: Some(unreachable_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some("ref:main")
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.version_tag.as_deref(),
        Some("main")
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(unreachable_commit.to_string()), None]
    );
}

#[test]
fn frozen_ref_pin_replays_locked_commit_exactly() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let locked_commit = "frozen-refpin-sha";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: Some(locked_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.version_tag.as_deref(),
        Some("main")
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
}

fn ref_pin_lock(version: Option<&str>, commit: Option<&str>) -> LockFile {
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: version.map(Into::into),
            commit: commit.map(Into::into),
            plugin: None,
            content: None,
        },
    );
    lock
}

fn ref_pin_provider(dir: &TempDir) -> MockProvider {
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();
    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);
    provider
}

#[test]
fn frozen_ref_pin_errors_without_a_usable_lock_entry() {
    // (requested ref, lock entry): missing entry, missing commit, selector mismatch,
    // missing selector.
    let cases = [
        ("main", None),
        ("main", Some(ref_pin_lock(Some("main"), None))),
        (
            "release",
            Some(ref_pin_lock(Some("main"), Some("stale-main-sha"))),
        ),
        ("main", Some(ref_pin_lock(None, Some("stale-main-sha")))),
    ];
    for (requested, lock) in cases {
        let dir = TempDir::new().unwrap();
        let provider = ref_pin_provider(&dir);
        let config = make_config(vec![(
            "a",
            git_spec("https://example.com/a.git", Some(requested)),
        )]);
        let lock = lock.unwrap_or_else(LockFile::empty);

        let result = resolve(&config, &provider, Some(&lock), &ResolveOptions::frozen());
        assert!(
            matches!(result, Err(MarsError::FrozenViolation { .. })),
            "{requested} with lock {:?}",
            lock.dependencies
        );
    }
}

#[test]
fn ref_pin_selector_mismatch_ignores_locked_commit_in_normal_sync() {
    // (requested ref, lock selector as the diagnostic names it). Either the config moved
    // from main to release, or the lock never recorded a selector.
    let cases = [
        ("release", Some("main"), "release"),
        ("main", None, "<missing>"),
    ];
    for (requested, locked_version, diagnostic_fragment) in cases {
        let dir = TempDir::new().unwrap();
        let provider = ref_pin_provider(&dir);
        let config = make_config(vec![(
            "a",
            git_spec("https://example.com/a.git", Some(requested)),
        )]);
        let lock = ref_pin_lock(locked_version, Some("stale-main-sha"));

        let (result, diagnostics) =
            resolve_with_diagnostics(&config, &provider, Some(&lock), &default_options());
        let graph = result.unwrap();
        assert_eq!(
            graph.nodes["a"].resolved_ref.commit.as_deref(),
            Some(format!("ref:{requested}").as_str()),
            "selector mismatch must skip lock replay and fetch requested ref live"
        );
        assert_eq!(
            graph.nodes["a"].resolved_ref.version_tag.as_deref(),
            Some(requested)
        );
        assert_eq!(provider.seen_preferred_commits(), vec![None]);
        assert!(
            diagnostics.iter().any(|diag| {
                diag.code == "locked-ref-selector-mismatch"
                    && diag.message.contains(diagnostic_fragment)
            }),
            "{requested}: missing mismatch diagnostic"
        );
    }
}

#[test]
fn frozen_ref_pin_errors_when_locked_commit_unreachable() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let unreachable_commit = "frozen-refpin-missing";
    provider.mark_unreachable_preferred_commit(unreachable_commit);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
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
fn maximize_with_specific_targets_replays_non_target_lock_entry() {
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

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "b".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/b.git".into()),
            path: None,
            subpath: None,
            version: Some("v2.0.0".into()),
            commit: Some("b-locked-sha".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::upgrade(HashSet::from(["a".into()]), false);

    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 5, 0)),
        "upgrade target should maximize"
    );
    assert_eq!(
        graph.nodes["b"].resolved_ref.version,
        Some(Version::new(2, 0, 0)),
        "non-target should replay locked version"
    );
    assert_eq!(
        graph.nodes["b"].resolved_ref.commit.as_deref(),
        Some("b-locked-sha"),
        "non-target should replay locked commit hint"
    );
}

#[test]
fn maximize_with_specific_targets_uses_latest_compatible_for_unlocked_non_target() {
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

    let options = ResolveOptions::upgrade(HashSet::from(["a".into()]), false);

    let graph = resolve(&config, &provider, None, &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 5, 0)),
        "upgrade target should maximize"
    );
    assert_eq!(
        graph.nodes["b"].resolved_ref.version,
        Some(Version::new(2, 5, 0)),
        "unlocked non-target should use latest-compatible fallback, not old MVS minimum"
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

    let options = ResolveOptions::upgrade(HashSet::from([SourceName::from("a")]), true);

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
            plugin: None,
            content: None,
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
            plugin: None,
            content: None,
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
fn frozen_mode_replays_unversioned_locked_commit_even_when_tags_now_exist() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    let url = "https://example.com/a.git";
    provider.add_versions(url, vec![(1, 0, 0), (2, 0, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec(url, None))]);

    let locked_commit = "frozen-untagged-sha";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some(url.into()),
            path: None,
            subpath: None,
            version: None,
            commit: Some(locked_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert!(graph.nodes["a"].resolved_ref.version.is_none());
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
    assert_eq!(
        provider.list_versions_count(url),
        0,
        "frozen replay should not list tags when lock already gives exact commit"
    );
}

#[test]
fn frozen_mode_errors_when_locked_semver_is_incompatible() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_versions(
        "https://example.com/a.git",
        vec![(1, 0, 0), (2, 0, 0), (2, 1, 0)],
    );
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("^2.0")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("old-commit".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
}

#[test]
fn frozen_semver_malformed_lock_fails_before_listing_versions() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let url = "https://example.com/a.git";
    let config = make_config(vec![("a", git_spec(url, Some("^1.0")))]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some(url.into()),
            path: None,
            subpath: None,
            version: Some("not-a-semver".into()),
            commit: Some("a-commit".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
    assert_eq!(provider.list_versions_count(url), 0);
}

#[test]
fn frozen_semver_replays_locked_commit_even_when_tag_missing_from_remote() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    let url = "https://example.com/a.git";
    provider.add_versions(url, vec![(2, 0, 0)]);
    provider.add_source("a", tree, None);

    let config = make_config(vec![("a", git_spec(url, Some("^1.0")))]);

    let locked_commit = "frozen-locked-sha";
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some(url.into()),
            path: None,
            subpath: None,
            version: Some("v1.1.0".into()),
            commit: Some(locked_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let graph = resolve(&config, &provider, Some(&lock), &options).unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.version,
        Some(Version::new(1, 1, 0))
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some(locked_commit)
    );
    assert_eq!(
        provider.seen_preferred_commits(),
        vec![Some(locked_commit.to_string())]
    );
    assert_eq!(provider.list_versions_count(url), 0);
}

#[test]
fn frozen_mode_errors_when_transitive_lock_entry_is_missing() {
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
    provider.add_versions("https://example.com/shared.git", vec![(1, 0, 0)]);
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
            commit: Some("a-locked".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
}

// R3: Transitive deps replay the consumer lock in normal sync.
#[test]
fn r3_transitive_dep_replays_consumer_lock() {
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

    // Lock records shared@v1.2.0, and shared is transitive.
    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("v1.0.0".into()),
            commit: Some("a-commit".into()),
            plugin: None,
            content: None,
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
            plugin: None,
            content: None,
        },
    );

    let graph = resolve(&config, &provider, Some(&lock), &default_options()).unwrap();
    // Transitive lock is replayed in normal sync.
    assert_eq!(
        graph.nodes["shared"].resolved_ref.version,
        Some(Version::new(1, 2, 0)),
        "transitive lock version v1.2.0 must be preserved"
    );
    // Locked commit should be replayed when version matches lock.
    let commits = provider.seen_preferred_commits();
    let shared_preferred = commits.last().cloned().flatten();
    assert_eq!(
        shared_preferred.as_deref(),
        Some("shared-locked-commit"),
        "transitive dep should receive locked commit hint"
    );
}

// R7: Direct dep without --frozen prefers locked version when constraint is satisfied.
// Covered by `locked_version_preferred_when_satisfies_constraint` above.

// R8: `mars upgrade <source>` maximizes only the named source; others keep lock-preferred/latest-compatible behavior.
// Covered by `maximize_with_specific_targets` above.

// R9: `mars upgrade` (no targets) maximizes all directs.
// Covered by `maximize_mode_picks_newest` above.

// R6: Lock records all resolved packages (direct + transitive) on first write; re-sync replays them.
// Covered by `upgrade_then_sync_keeps_upgraded_transitive_lock_and_content` in `tests/sync_behavior.rs`.

// R10: A source listed as both direct and transitive replays the lock regardless of encounter order.

// R11: Multi-intermediary empty constraint intersection → error naming constraints and sources.
// Covered by `latest_constraint_does_not_skip_sibling_semver_validation` above.

// R12: RefPin overrides semver version selection.
// Covered by `branch_ref_resolves_without_semver` above.

// R13: Multiple different RefPins for the same source conflict.
#[test]
fn r13_multiple_refpins_conflict() {
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

    // Two different pins on one package are a conflict, whichever declaration meets them first.
    let error = resolve(&config, &provider, None, &default_options()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("ref:main") && message.contains("ref:dev"),
        "conflict must name both pins: {message}"
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
            plugin: None,
            content: None,
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
