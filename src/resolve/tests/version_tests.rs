use super::*;

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

#[test]
fn frozen_ref_pin_errors_when_lock_entry_missing() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&LockFile::empty()), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
}

#[test]
fn frozen_ref_pin_errors_when_locked_commit_missing() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: None,
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
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
fn ref_pin_selector_mismatch_ignores_locked_commit_in_normal_sync() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    // Config changed from main -> release, but lock still records main.
    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("release")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: Some("stale-main-sha".into()),
            plugin: None,
            content: None,
        },
    );

    let (result, diagnostics) =
        resolve_with_diagnostics(&config, &provider, Some(&lock), &default_options());
    let graph = result.unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some("ref:release"),
        "selector mismatch must skip lock replay and fetch requested ref live"
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.version_tag.as_deref(),
        Some("release")
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
    assert!(diagnostics.iter().any(|diag| {
        diag.code == "locked-ref-selector-mismatch" && diag.message.contains("release")
    }));
}

#[test]
fn ref_pin_selector_missing_ignores_locked_commit_in_normal_sync() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: None,
            commit: Some("stale-main-sha".into()),
            plugin: None,
            content: None,
        },
    );

    let (result, diagnostics) =
        resolve_with_diagnostics(&config, &provider, Some(&lock), &default_options());
    let graph = result.unwrap();
    assert_eq!(
        graph.nodes["a"].resolved_ref.commit.as_deref(),
        Some("ref:main")
    );
    assert_eq!(
        graph.nodes["a"].resolved_ref.version_tag.as_deref(),
        Some("main")
    );
    assert_eq!(provider.seen_preferred_commits(), vec![None]);
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.code == "locked-ref-selector-mismatch"
                && diag.message.contains("<missing>"))
    );
}

#[test]
fn frozen_ref_pin_errors_when_selector_mismatches_requested_ref() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("release")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: Some("main".into()),
            commit: Some("stale-main-sha".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
}

#[test]
fn frozen_ref_pin_errors_when_selector_missing() {
    let dir = TempDir::new().unwrap();
    let tree = dir.path().join("a");
    std::fs::create_dir_all(&tree).unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("a", tree, None);

    let config = make_config(vec![(
        "a",
        git_spec("https://example.com/a.git", Some("main")),
    )]);

    let mut lock = LockFile::empty();
    lock.dependencies.insert(
        "a".into(),
        crate::lock::LockedSource {
            url: Some("https://example.com/a.git".into()),
            path: None,
            subpath: None,
            version: None,
            commit: Some("stale-main-sha".into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(matches!(result, Err(MarsError::FrozenViolation { .. })));
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

// ========== EARS R1–R15: resolver lock semantics ==========

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
            version: Some("v1.0.0".into()),
            commit: Some(unreachable.into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
    // With frozen, transitive deps also consult the lock → unreachable commit → error.
    let result = resolve(&config, &provider, Some(&lock), &options);
    assert!(
        matches!(result, Err(MarsError::LockedCommitUnreachable { .. })),
        "frozen + unreachable transitive locked commit should error: {result:?}"
    );
}

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
            version: Some("v1.1.0".into()),
            commit: Some(locked_commit.into()),
            plugin: None,
            content: None,
        },
    );

    let options = ResolveOptions::frozen();
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
