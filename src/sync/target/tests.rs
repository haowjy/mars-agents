use super::*;
use crate::config::*;
use crate::lock::LockFile;
use crate::provided::ProvidedNames;
use crate::resolve::{ResolvedGraph, ResolvedNode};
use crate::source::ResolvedRef;
use crate::types::SourceId;
use indexmap::IndexMap;
use mars_schema::filter::FilterMode;
use std::fs;
use tempfile::TempDir;

/// The consumer's own package in a dependency-only test graph.
fn test_local_source() -> SourceName {
    crate::types::SourceOrigin::LocalPackage.to_string().into()
}

/// Helper: create a source tree with agents and skills
fn make_source_tree(agents: &[(&str, &str)], skills: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    if !agents.is_empty() {
        let agents_dir = dir.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        for (name, content) in agents {
            fs::write(agents_dir.join(name), content).unwrap();
        }
    }
    if !skills.is_empty() {
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        for (name, content) in skills {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(skill_dir.join("SKILL.md"), content).unwrap();
        }
    }
    dir
}

fn make_graph_and_config(
    sources: Vec<(&str, &TempDir, Option<&str>, FilterMode)>,
) -> (ResolvedGraph, EffectiveConfig) {
    let mut nodes = IndexMap::new();
    let mut order = Vec::new();
    let mut config_dependencies = IndexMap::new();

    for (name, tree, url, filter) in sources {
        nodes.insert(
            name.into(),
            ResolvedNode {
                fetch_url: None,
                source_name: name.into(),
                source_id: if let Some(u) = url {
                    SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                } else {
                    SourceId::Path {
                        canonical: tree.path().to_path_buf(),
                        subpath: None,
                        plugin: None,
                    }
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: tree.path().to_path_buf(),
                    authored_root: tree.path().to_path_buf(),
                    package_root: tree.path().to_path_buf(),
                    plugin: None,
                },
                prepared: crate::package_prepare::prepare_tree_for_test(name, tree.path()),
                resolved_ref: ResolvedRef {
                    source_name: name.into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: tree.path().to_path_buf(),
                },
                manifest: None,
            },
        );
        order.push(name.into());

        let spec = if let Some(u) = url {
            SourceSpec::Git(GitSpec {
                url: crate::types::SourceUrl::from(u),
                version: None,
            })
        } else {
            SourceSpec::Path(tree.path().to_path_buf())
        };

        config_dependencies.insert(
            name.into(),
            EffectiveDependency {
                declared_source_id: if let Some(u) = url {
                    SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                } else {
                    SourceId::Path {
                        canonical: tree.path().to_path_buf(),
                        subpath: None,
                        plugin: None,
                    }
                },
                source_id: if let Some(u) = url {
                    SourceId::git_with_subpath(crate::types::SourceUrl::from(u), None)
                } else {
                    SourceId::Path {
                        canonical: tree.path().to_path_buf(),
                        subpath: None,
                        plugin: None,
                    }
                },
                spec,
                subpath: None,
                filter,
                rename: RenameMap::new(),
                dialect: None,
            },
        );
    }

    let config = EffectiveConfig {
        dependencies: config_dependencies,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };
    let graph = ResolvedGraph {
        selection: crate::resolve::select_for_test(&config, &nodes),
        nodes,
        order,
        version_constraints: std::collections::HashMap::new(),
        unreadable_hook_surfaces: std::collections::BTreeMap::new(),
    };
    (graph, config)
}

// === Target build tests ===

#[test]
#[cfg(not(target_os = "windows"))]
fn invalid_windows_agent_filename_emits_diagnostic_and_skips() {
    // This test creates a file with `:` in the name, which is only possible on
    // non-Windows. The validation catches names that would break on Windows when
    // created on POSIX systems.
    let tree = make_source_tree(&[("bad:name.md", "# bad"), ("coder.md", "# coder")], &[]);
    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        Some("https://github.com/org/base"),
        FilterMode::All,
    )]);
    let mut diag = DiagnosticCollector::new();

    let (target, _, _, _) =
        build_with_collisions_and_diag(&graph, &config, &test_local_source(), &mut diag).unwrap();
    let diagnostics = diag.drain();

    assert!(!target.items.contains_key("agents/bad:name.md"));
    assert!(target.items.contains_key("agents/coder.md"));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "invalid-agent-filename");
}

#[test]
fn parse_rename_dest_rejects_absolute_and_escape_destinations() {
    let source_name = SourceName::from("base");

    let absolute = parse_rename_dest(RenamableKind::Agent, "/tmp/escape", &source_name)
        .expect_err("absolute rename should fail");
    assert!(matches!(absolute, MarsError::Source { .. }));

    let traversal = parse_rename_dest(RenamableKind::Skill, "../escape", &source_name)
        .expect_err("traversal rename should fail");
    assert!(matches!(traversal, MarsError::Source { .. }));
}

#[test]
fn build_with_invalid_rename_destination_returns_error() {
    let tree = make_source_tree(&[("old-name.md", "# old")], &[]);

    let (graph, mut config) = make_graph_and_config(vec![("base", &tree, None, FilterMode::All)]);

    config
        .dependencies
        .get_mut("base")
        .unwrap()
        .rename
        .insert("agents/old-name.md".into(), "../escape.md".into());

    let err = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap_err();
    assert!(matches!(err, MarsError::Source { .. }));
}

// === Collision tests ===

#[test]
fn same_source_explicit_rename_collision_stays_hard_error() {
    let tree = make_source_tree(
        &[
            ("coder.md", "# coder"),
            ("reviewer.md", "# reviewer renamed into coder"),
        ],
        &[],
    );
    let (graph, mut config) =
        make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
    config
        .dependencies
        .get_mut("source-a")
        .unwrap()
        .rename
        .insert("agents/reviewer.md".into(), "agents/coder.md".into());

    let err = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap_err();

    assert!(matches!(err, MarsError::Collision { .. }));
}

#[test]
fn mixed_kind_explicit_rename_collision_stays_hard_error() {
    let tree = make_source_tree(&[("coder.md", "# coder")], &[("planning", "# planning")]);
    let (graph, mut config) =
        make_graph_and_config(vec![("source-a", &tree, None, FilterMode::All)]);
    config
        .dependencies
        .get_mut("source-a")
        .unwrap()
        .rename
        .insert("agents/coder.md".into(), "skills/planning".into());

    let err = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap_err();

    assert!(matches!(err, MarsError::Collision { .. }));
}

#[test]
fn duplicate_source_in_cross_source_group_stays_hard_error_without_auto_warning() {
    let tree1 = make_source_tree(
        &[
            ("coder.md", "# coder"),
            ("reviewer.md", "# reviewer renamed into coder"),
        ],
        &[],
    );
    let tree2 = make_source_tree(&[("coder.md", "# coder from source 2")], &[]);
    let (graph, mut config) = make_graph_and_config(vec![
        ("source-a", &tree1, None, FilterMode::All),
        ("source-b", &tree2, None, FilterMode::All),
    ]);
    config
        .dependencies
        .get_mut("source-a")
        .unwrap()
        .rename
        .insert("agents/reviewer.md".into(), "agents/coder.md".into());
    let mut diag = DiagnosticCollector::new();

    let err = build_with_collisions_and_diag(&graph, &config, &test_local_source(), &mut diag)
        .unwrap_err();
    let diagnostics = diag.drain();

    assert!(matches!(err, MarsError::Collision { .. }));
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "auto-rename-collision")
    );
}

// === Source with agents filter + skill deps ===

#[test]
fn build_with_agents_filter_pulls_transitive_skills() {
    let tree = make_source_tree(
        &[("coder.md", "---\nskills:\n  - planning\n---\n# Coder\n")],
        &[("planning", "# Planning"), ("unused-skill", "# Unused")],
    );

    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        None,
        FilterMode::Include {
            agents: vec!["coder".into()],
            skills: vec![],
        },
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    assert_eq!(target.items.len(), 2); // coder + planning
    assert!(target.items.contains_key("agents/coder.md"));
    assert!(target.items.contains_key("skills/planning"));
    // unused-skill should NOT be present
    assert!(!target.items.contains_key("skills/unused-skill"));
}

#[test]
fn build_with_exclude_filter() {
    let tree = make_source_tree(&[("coder.md", "# coder"), ("deprecated.md", "# old")], &[]);

    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        None,
        FilterMode::Exclude(vec!["deprecated".into()]),
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    assert_eq!(target.items.len(), 1);
    assert!(target.items.contains_key("agents/coder.md"));
}

#[test]
fn unmanaged_disk_path_collision_reported() {
    let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        Some("https://github.com/org/base"),
        FilterMode::All,
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    let install_root = TempDir::new().unwrap();

    // Existing user-authored file at the same destination.
    let existing = install_root.path().join("agents").join("coder.md");
    fs::create_dir_all(existing.parent().unwrap()).unwrap();
    fs::write(&existing, "# user-authored").unwrap();

    let collisions = check_unmanaged_collisions(
        install_root.path(),
        &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
        &target,
        false,
    );
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].source_name.as_ref(), "base");
    assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
}

#[test]
fn unmanaged_collision_skipped_when_hash_matches() {
    let content = "# managed agent";
    let tree = make_source_tree(&[("coder.md", content)], &[]);
    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        Some("https://github.com/org/base"),
        FilterMode::All,
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    let install_root = TempDir::new().unwrap();

    // Simulate partial prior install: file on disk with same content
    let existing = install_root.path().join("agents").join("coder.md");
    fs::create_dir_all(existing.parent().unwrap()).unwrap();
    fs::write(&existing, content).unwrap();

    // Should skip collision — disk content matches planned install (crash recovery)
    let collisions = check_unmanaged_collisions(
        install_root.path(),
        &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
        &target,
        false,
    );
    assert!(collisions.is_empty());
}

#[test]
fn unmanaged_collision_reported_on_different_content() {
    let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        Some("https://github.com/org/base"),
        FilterMode::All,
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    let install_root = TempDir::new().unwrap();

    // User-authored file with different content
    let existing = install_root.path().join("agents").join("coder.md");
    fs::create_dir_all(existing.parent().unwrap()).unwrap();
    fs::write(&existing, "# different user content").unwrap();

    let collisions = check_unmanaged_collisions(
        install_root.path(),
        &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
        &target,
        false,
    );
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].source_name.as_ref(), "base");
    assert_eq!(collisions[0].path.as_str(), "agents/coder.md");
}

#[test]
fn unmanaged_collision_skipped_under_force() {
    let tree = make_source_tree(&[("coder.md", "# managed")], &[]);
    let (graph, config) = make_graph_and_config(vec![(
        "base",
        &tree,
        Some("https://github.com/org/base"),
        FilterMode::All,
    )]);

    let (target, renames, _, _) = build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    assert!(renames.is_empty());
    let install_root = TempDir::new().unwrap();

    let existing = install_root.path().join("agents").join("coder.md");
    fs::create_dir_all(existing.parent().unwrap()).unwrap();
    fs::write(&existing, "# stale cache content").unwrap();

    let collisions = check_unmanaged_collisions(
        install_root.path(),
        &Ownership::new(&LockFile::empty(), &ProvidedNames::default()),
        &target,
        true,
    );
    assert!(collisions.is_empty());
}
