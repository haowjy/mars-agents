//! Tests for the sync pipeline: target building, validation, planning, and apply.

use super::*;
use crate::config::*;
use crate::lock::{ItemKind, LockFile};
use crate::resolve::{ResolvedGraph, ResolvedNode};
use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Helper to set up a complete sync context with temp dirs.
struct TestFixture {
    project_root: TempDir,
    managed_root: PathBuf,
    source_trees: Vec<TempDir>,
}

impl TestFixture {
    fn new() -> Self {
        let project_root = TempDir::new().unwrap();
        let managed_root = project_root.path().join(".agents");
        // Create .mars/cache directories
        fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
        TestFixture {
            project_root,
            managed_root,
            source_trees: Vec::new(),
        }
    }

    fn add_source(&mut self, agents: &[(&str, &str)], skills: &[(&str, &str)]) -> usize {
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
        self.source_trees.push(dir);
        self.source_trees.len() - 1
    }

    fn project_root(&self) -> &std::path::Path {
        self.project_root.path()
    }

    fn managed_root(&self) -> &std::path::Path {
        &self.managed_root
    }

    fn tree_path(&self, idx: usize) -> PathBuf {
        self.source_trees[idx].path().to_path_buf()
    }
}

fn make_graph_config(
    fixture: &TestFixture,
    sources: Vec<(&str, usize, FilterMode)>,
) -> (ResolvedGraph, EffectiveConfig) {
    let mut nodes = IndexMap::new();
    let mut order = Vec::new();
    let mut config_dependencies = IndexMap::new();

    for (name, tree_idx, filter) in sources {
        let tree_path = fixture.tree_path(tree_idx);
        nodes.insert(
            name.into(),
            ResolvedNode {
                source_name: name.into(),
                source_id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: tree_path.clone(),
                    package_root: tree_path.clone(),
                },
                resolved_ref: crate::source::ResolvedRef {
                    source_name: name.into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: tree_path.clone(),
                },
                latest_version: None,
                manifest: None,
                deps: vec![],
            },
        );
        order.push(name.into());

        config_dependencies.insert(
            name.into(),
            EffectiveDependency {
                name: name.into(),
                id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                },
                spec: SourceSpec::Path(tree_path),
                subpath: None,
                filter,
                rename: crate::types::RenameMap::new(),
                dialect: None,
                is_overridden: false,
                original_git: None,
            },
        );
    }

    (
        ResolvedGraph {
            nodes,
            order,
            filters: std::collections::HashMap::new(),
            version_constraints: std::collections::HashMap::new(),
        },
        EffectiveConfig {
            dependencies: config_dependencies,
            settings: Settings::default(),
            skills: indexmap::IndexMap::new(),
        },
    )
}

fn path_dependency_entry(path: &std::path::Path) -> DependencyEntry {
    DependencyEntry {
        url: None,
        path: Some(path.to_path_buf()),
        subpath: None,
        version: None,
        dialect: None,
        filter: FilterConfig::default(),
    }
}

fn git_dependency_entry(url: &str, version: &str, filter: FilterConfig) -> DependencyEntry {
    DependencyEntry {
        url: Some(url.into()),
        path: None,
        subpath: None,
        version: Some(version.to_string()),
        dialect: None,
        filter,
    }
}

fn create_sync_plan(
    sync_diff: &diff::SyncDiff,
    options: &SyncOptions,
    cache_bases_dir: &std::path::Path,
) -> plan::SyncPlan {
    let mut diag = DiagnosticCollector::new();
    plan::create(sync_diff, options, cache_bases_dir, &mut diag)
}

#[test]
fn build_target_prunes_unmanaged_collision_before_rewriting_refs() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(
        &[("coder.md", "---\nskills: [planning]\n---\n# Agent\n")],
        &[("planning", "# Planning A")],
    );
    let source_b = fixture.add_source(&[], &[("planning", "# Planning B")]);
    let (mut graph, mut effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
    graph
        .nodes
        .get_mut(&SourceName::from("source-a"))
        .unwrap()
        .deps = vec!["source-b".into()];
    effective
        .dependencies
        .get_mut(&SourceName::from("source-b"))
        .unwrap()
        .rename
        .insert("planning".into(), "planning__source-b".into());

    let unmanaged_skill = fixture.project_root().join(".mars/skills/planning");
    fs::create_dir_all(&unmanaged_skill).unwrap();
    fs::write(unmanaged_skill.join("SKILL.md"), "# unmanaged").unwrap();

    let sync_lock =
        crate::fs::FileLock::acquire(&fixture.project_root().join(".mars/sync.lock")).unwrap();
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config: Config::default(),
            local: LocalConfig::default(),
            effective,
            old_lock: LockFile::empty(),
            dependency_changes: Vec::new(),
            sync_lock,
        },
        graph,
        upgrades_available: 0,
    };
    let ctx = MarsContext::for_test(
        fixture.project_root().to_path_buf(),
        fixture.managed_root().to_path_buf(),
    );
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    let targeted = build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    assert!(!targeted.target.items.contains_key("skills/planning"));
    let rewritten = targeted.target.items["agents/coder.md"]
        .rewritten_content
        .as_ref()
        .expect("agent ref should rewrite after unmanaged source skill is pruned");
    let fm = crate::frontmatter::parse(rewritten).unwrap();
    assert_eq!(fm.skills(), vec!["planning__source-b"]);
}

#[test]
fn build_target_warns_when_fanout_references_collision_renamed_agent() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(
        &[("web-researcher.md", "---\nname: web-researcher\n---\n# A\n")],
        &[],
    );
    let source_b = fixture.add_source(
        &[("web-researcher.md", "---\nname: web-researcher\n---\n# B\n")],
        &[],
    );
    let (graph, mut effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
    effective.settings.meridian.fanout = Some(crate::config::FanoutConfig {
        agents: vec!["web-researcher".into()],
    });

    let sync_lock =
        crate::fs::FileLock::acquire(&fixture.project_root().join(".mars/sync.lock")).unwrap();
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config: Config::default(),
            local: LocalConfig::default(),
            effective,
            old_lock: LockFile::empty(),
            dependency_changes: Vec::new(),
            sync_lock,
        },
        graph,
        upgrades_available: 0,
    };
    let ctx = MarsContext::for_test(
        fixture.project_root().to_path_buf(),
        fixture.managed_root().to_path_buf(),
    );
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    let dangles: Vec<String> = diag
        .drain()
        .into_iter()
        .filter(|d| d.code == "config-rename-dangle")
        .map(|d| d.message)
        .collect();
    assert_eq!(
        dangles.len(),
        1,
        "expected one config-rename-dangle warning, got: {dangles:?}"
    );
    assert!(
        dangles[0].contains("web-researcher"),
        "should name the dangled agent: {}",
        dangles[0]
    );
    assert!(
        dangles[0].contains("web-researcher__source-a")
            && dangles[0].contains("web-researcher__source-b"),
        "should list the new installed names: {}",
        dangles[0]
    );
    assert!(
        dangles[0].contains("[settings.meridian.fanout].agents"),
        "should name the config location: {}",
        dangles[0]
    );
}

#[test]
fn build_target_warns_when_agent_overlay_references_collision_renamed_agent() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(&[("coder.md", "---\nname: coder\n---\n# A\n")], &[]);
    let source_b = fixture.add_source(&[("coder.md", "---\nname: coder\n---\n# B\n")], &[]);
    let (graph, effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
    let mut config = Config::default();
    config.agents.insert(
        "coder".into(),
        crate::config::AgentOverlay {
            model: Some("gpt55".into()),
            ..Default::default()
        },
    );

    let sync_lock =
        crate::fs::FileLock::acquire(&fixture.project_root().join(".mars/sync.lock")).unwrap();
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config,
            local: LocalConfig::default(),
            effective,
            old_lock: LockFile::empty(),
            dependency_changes: Vec::new(),
            sync_lock,
        },
        graph,
        upgrades_available: 0,
    };
    let ctx = MarsContext::for_test(
        fixture.project_root().to_path_buf(),
        fixture.managed_root().to_path_buf(),
    );
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    let dangles: Vec<String> = diag
        .drain()
        .into_iter()
        .filter(|d| d.code == "config-rename-dangle")
        .map(|d| d.message)
        .collect();
    assert_eq!(dangles.len(), 1, "got: {dangles:?}");
    assert!(dangles[0].contains("coder"), "{}", dangles[0]);
    assert!(dangles[0].contains("[agents.<name>]"), "{}", dangles[0]);
}

#[test]
fn build_target_warns_when_skill_overlay_references_explicitly_renamed_skill() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(&[], &[("planning", "# Planning A")]);
    let (graph, mut effective) =
        make_graph_config(&fixture, vec![("source-a", source_a, FilterMode::All)]);
    effective
        .dependencies
        .get_mut(&SourceName::from("source-a"))
        .unwrap()
        .rename
        .insert("planning".into(), "strategy".into());
    effective.skills.insert(
        "planning".into(),
        crate::config::SkillOverlay {
            model_invocable: Some(false),
            ..Default::default()
        },
    );

    let sync_lock =
        crate::fs::FileLock::acquire(&fixture.project_root().join(".mars/sync.lock")).unwrap();
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config: Config::default(),
            local: LocalConfig::default(),
            effective,
            old_lock: LockFile::empty(),
            dependency_changes: Vec::new(),
            sync_lock,
        },
        graph,
        upgrades_available: 0,
    };
    let ctx = MarsContext::for_test(
        fixture.project_root().to_path_buf(),
        fixture.managed_root().to_path_buf(),
    );
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    let dangles: Vec<String> = diag
        .drain()
        .into_iter()
        .filter(|d| d.code == "config-rename-dangle")
        .map(|d| d.message)
        .collect();
    assert_eq!(dangles.len(), 1, "got: {dangles:?}");
    assert!(dangles[0].contains("planning"), "{}", dangles[0]);
    assert!(dangles[0].contains("[skills.<name>]"), "{}", dangles[0]);
}

#[test]
fn build_target_does_not_warn_when_fanout_references_unrenamed_agent() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(
        &[("orchestrator.md", "---\nname: orchestrator\n---\n# A\n")],
        &[],
    );
    let source_b = fixture.add_source(
        &[("web-researcher.md", "---\nname: web-researcher\n---\n# B\n")],
        &[],
    );
    let (graph, mut effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
    effective.settings.meridian.fanout = Some(crate::config::FanoutConfig {
        agents: vec!["orchestrator".into()],
    });

    let sync_lock =
        crate::fs::FileLock::acquire(&fixture.project_root().join(".mars/sync.lock")).unwrap();
    let resolved = ResolvedState {
        loaded: LoadedConfig {
            config: Config::default(),
            local: LocalConfig::default(),
            effective,
            old_lock: LockFile::empty(),
            dependency_changes: Vec::new(),
            sync_lock,
        },
        graph,
        upgrades_available: 0,
    };
    let ctx = MarsContext::for_test(
        fixture.project_root().to_path_buf(),
        fixture.managed_root().to_path_buf(),
    );
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    let dangles: Vec<String> = diag
        .drain()
        .into_iter()
        .filter(|d| d.code == "config-rename-dangle")
        .map(|d| d.message)
        .collect();
    assert!(
        dangles.is_empty(),
        "no dangle expected for unrenamed ref: {dangles:?}"
    );
}

fn graph_with_versions(entries: &[(&str, &str, &str)]) -> ResolvedGraph {
    let mut nodes = IndexMap::new();
    let mut order = Vec::new();
    for (name, url, tag) in entries {
        let version = semver::Version::parse(tag.trim_start_matches('v')).unwrap();
        nodes.insert(
            (*name).into(),
            ResolvedNode {
                source_name: (*name).into(),
                source_id: crate::types::SourceId::git(crate::types::SourceUrl::from(*url)),
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: PathBuf::from(format!("/tmp/{name}")),
                    package_root: PathBuf::from(format!("/tmp/{name}")),
                },
                resolved_ref: crate::source::ResolvedRef {
                    source_name: (*name).into(),
                    version: Some(version),
                    version_tag: Some((*tag).to_string()),
                    commit: Some("abc123".into()),
                    tree_path: PathBuf::from(format!("/tmp/{name}")),
                },
                latest_version: None,
                manifest: None,
                deps: vec![],
            },
        );
        order.push((*name).into());
    }

    ResolvedGraph {
        nodes,
        order,
        filters: std::collections::HashMap::new(),
        version_constraints: std::collections::HashMap::new(),
    }
}

#[test]
fn validate_request_rejects_frozen_with_maximize() {
    let request = SyncRequest {
        resolution: ResolutionMode::Maximize {
            targets: HashSet::new(),
            bump: false,
        },
        mutation: None,
        options: SyncOptions {
            frozen: true,
            ..SyncOptions::default()
        },
        lossiness_mode: LossinessMode::Hidden,
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, MarsError::InvalidRequest { .. }));
    assert!(err.to_string().contains("--frozen"));
}

#[test]
fn validate_request_rejects_frozen_with_mutation() {
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: Some(ConfigMutation::RemoveDependency {
            name: "base".into(),
        }),
        options: SyncOptions {
            frozen: true,
            ..SyncOptions::default()
        },
        lossiness_mode: LossinessMode::Hidden,
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, MarsError::InvalidRequest { .. }));
    assert!(err.to_string().contains("cannot modify config"));
}

#[test]
fn planned_bump_entries_bump_all_outdated_pins() {
    let mut config = Config::default();
    config.dependencies.insert(
        "base".into(),
        git_dependency_entry(
            "https://example.com/base.git",
            "v1.0.0",
            FilterConfig::default(),
        ),
    );
    config.dependencies.insert(
        "tools".into(),
        git_dependency_entry(
            "https://example.com/tools.git",
            "v2.0.0",
            FilterConfig::default(),
        ),
    );
    config.dependencies.insert(
        "floating".into(),
        DependencyEntry {
            url: Some("https://example.com/floating.git".into()),
            path: None,
            subpath: None,
            version: None,
            dialect: None,
            filter: FilterConfig::default(),
        },
    );

    let graph = graph_with_versions(&[
        ("base", "https://example.com/base.git", "v1.2.0"),
        ("tools", "https://example.com/tools.git", "v2.0.0"),
        ("floating", "https://example.com/floating.git", "v3.0.0"),
    ]);

    let mode = ResolutionMode::Maximize {
        targets: HashSet::new(),
        bump: true,
    };
    let entries = planned_bump_entries(&config, &graph, &mode);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, SourceName::from("base"));
    assert_eq!(entries[0].1.version.as_deref(), Some("v1.2.0"));
}

#[test]
fn planned_bump_entries_bump_specific_targets_only() {
    let mut config = Config::default();
    config.dependencies.insert(
        "base".into(),
        git_dependency_entry(
            "https://example.com/base.git",
            "v1.0.0",
            FilterConfig::default(),
        ),
    );
    config.dependencies.insert(
        "tools".into(),
        git_dependency_entry(
            "https://example.com/tools.git",
            "v1.0.0",
            FilterConfig::default(),
        ),
    );

    let graph = graph_with_versions(&[
        ("base", "https://example.com/base.git", "v2.0.0"),
        ("tools", "https://example.com/tools.git", "v2.0.0"),
    ]);

    let mode = ResolutionMode::Maximize {
        targets: HashSet::from([SourceName::from("tools")]),
        bump: true,
    };
    let entries = planned_bump_entries(&config, &graph, &mode);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, SourceName::from("tools"));
    assert_eq!(entries[0].1.version.as_deref(), Some("v2.0.0"));
}

#[test]
fn planned_bump_entries_noop_when_already_latest() {
    let mut config = Config::default();
    config.dependencies.insert(
        "base".into(),
        git_dependency_entry(
            "https://example.com/base.git",
            "v1.2.0",
            FilterConfig::default(),
        ),
    );

    let graph = graph_with_versions(&[("base", "https://example.com/base.git", "v1.2.0")]);

    let mode = ResolutionMode::Maximize {
        targets: HashSet::new(),
        bump: true,
    };
    let entries = planned_bump_entries(&config, &graph, &mode);
    assert!(entries.is_empty());
}

#[test]
fn planned_bump_entries_preserve_filters_and_renames() {
    let mut rename = crate::types::RenameMap::new();
    rename.insert("coder".into(), "coder-v2".into());

    let mut config = Config::default();
    config.dependencies.insert(
        "base".into(),
        git_dependency_entry(
            "https://example.com/base.git",
            "v1.0.0",
            FilterConfig {
                agents: Some(vec!["coder".into()]),
                rename: Some(rename.clone()),
                ..FilterConfig::default()
            },
        ),
    );

    let graph = graph_with_versions(&[("base", "https://example.com/base.git", "v2.0.0")]);
    let mode = ResolutionMode::Maximize {
        targets: HashSet::new(),
        bump: true,
    };
    let entries = planned_bump_entries(&config, &graph, &mode);
    let mut mutated = config.clone();
    let changes =
        mutation::apply_mutation(&mut mutated, &ConfigMutation::BatchUpsert(entries)).unwrap();

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].old_version.as_deref(), Some("v1.0.0"));
    assert_eq!(changes[0].new_version.as_deref(), Some("v2.0.0"));

    let dep = &mutated.dependencies["base"];
    assert_eq!(dep.version.as_deref(), Some("v2.0.0"));
    assert_eq!(dep.filter.agents.as_deref(), Some(&["coder".into()][..]));
    assert_eq!(dep.filter.rename.as_ref(), Some(&rename));
}

#[test]
fn execute_auto_inits_config_for_mutation() {
    let project_root = TempDir::new().unwrap();
    let managed_root = project_root.path().join(".agents");
    fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
    let source = TempDir::new().unwrap();
    fs::create_dir_all(source.path().join("agents")).unwrap();
    fs::write(source.path().join("agents/coder.md"), "# Coder").unwrap();

    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: Some(ConfigMutation::UpsertDependency {
            name: "base".into(),
            entry: path_dependency_entry(source.path()),
        }),
        options: SyncOptions::default(),
        lossiness_mode: LossinessMode::Hidden,
    };

    let ctx = MarsContext::for_test(project_root.path().to_path_buf(), managed_root.clone());
    let report = execute(&ctx, &request).unwrap();
    assert!(!report.applied.outcomes.is_empty());
    assert!(project_root.path().join("mars.toml").exists());

    let saved = crate::config::load(project_root.path()).unwrap();
    assert!(saved.dependencies.contains_key("base"));
}

#[test]
fn execute_dry_run_with_mutation_does_not_write_config() {
    let project_root = TempDir::new().unwrap();
    let managed_root = project_root.path().join(".agents");
    fs::create_dir_all(project_root.path().join(".mars/cache/bases")).unwrap();
    crate::config::save(
        project_root.path(),
        &Config {
            dependencies: IndexMap::new(),
            settings: Settings::default(),
            ..Config::default()
        },
    )
    .unwrap();

    let source = TempDir::new().unwrap();
    fs::create_dir_all(source.path().join("agents")).unwrap();
    fs::write(source.path().join("agents/coder.md"), "# Coder").unwrap();

    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: Some(ConfigMutation::UpsertDependency {
            name: "base".into(),
            entry: path_dependency_entry(source.path()),
        }),
        options: SyncOptions {
            dry_run: true,
            ..SyncOptions::default()
        },
        lossiness_mode: LossinessMode::Hidden,
    };

    let ctx = MarsContext::for_test(project_root.path().to_path_buf(), managed_root.clone());
    let report = execute(&ctx, &request).unwrap();
    assert!(!report.applied.outcomes.is_empty());

    let saved = crate::config::load(project_root.path()).unwrap();
    assert!(!saved.dependencies.contains_key("base"));
    assert!(!managed_root.join("agents/coder.md").exists());
    assert!(!project_root.path().join("mars.lock").exists());
}

// === Integration tests for the pipeline stages ===

#[test]
fn full_pipeline_fresh_sync() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(
        &[("coder.md", "# Coder agent")],
        &[("planning", "# Planning skill")],
    );

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // Build target
    let (target, renames, _) = target::build_with_collisions(&graph, &config).unwrap();
    assert!(renames.is_empty());
    assert_eq!(target.items.len(), 2);

    // Compute diff against empty lock
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();

    // All items should be Add
    assert_eq!(sync_diff.items.len(), 2);
    for entry in &sync_diff.items {
        assert!(matches!(entry, diff::DiffEntry::Add { .. }));
    }

    // Create plan
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    assert_eq!(sync_plan.actions.len(), 2);
    for action in &sync_plan.actions {
        assert!(matches!(action, plan::PlannedAction::Install { .. }));
    }

    // Execute plan
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    assert_eq!(result.outcomes.len(), 2);

    // Verify files were created
    assert!(fixture.managed_root().join("agents/coder.md").exists());
    assert!(
        fixture
            .managed_root()
            .join("skills/planning/SKILL.md")
            .exists()
    );

    // Build lock
    let new_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();
    assert_eq!(new_lock.items.len(), 2);
    assert!(new_lock.items.contains_key("agent/coder"));
    assert!(new_lock.items.contains_key("skill/planning"));
}

#[test]
fn re_sync_no_changes() {
    let mut fixture = TestFixture::new();
    let content = "# Coder agent";
    let src_idx = fixture.add_source(&[("coder.md", content)], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    // Second sync with same content
    let (target2, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    // All items should be Unchanged
    for entry in &sync_diff2.items {
        assert!(
            matches!(entry, diff::DiffEntry::Unchanged { .. }),
            "expected Unchanged, got {entry:?}"
        );
    }

    let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
    for action in &sync_plan2.actions {
        assert!(matches!(action, plan::PlannedAction::Skip { .. }));
    }
}

#[test]
fn sync_staging_overlay_dialect_unchanged_and_frozen_diff() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(
        &[],
        &[(
            "planning",
            "---\nname: planning\ndescription: base\ndisable-model-invocation: true\nuser-invocable: true\n---\n# Planning\n",
        )],
    );
    let tree_path = fixture.tree_path(src_idx);
    let staging_root = fixture.project_root().join(".mars/staging");
    fs::create_dir_all(&staging_root).unwrap();

    let mut config = EffectiveConfig {
        dependencies: indexmap::IndexMap::from([(
            "base".into(),
            EffectiveDependency {
                name: "base".into(),
                id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                },
                spec: SourceSpec::Path(tree_path.clone()),
                subpath: None,
                filter: FilterMode::All,
                rename: crate::types::RenameMap::new(),
                dialect: Some(crate::dialect::Dialect::Claude),
                is_overridden: false,
                original_git: None,
            },
        )]),
        settings: Settings::default(),
        skills: indexmap::IndexMap::new(),
    };

    let stage = |cfg: &EffectiveConfig| {
        let mut diag = DiagnosticCollector::new();
        crate::staging::stage_rooted_source(
            &"base".into(),
            crate::resolve::RootedSourceRef {
                checkout_root: tree_path.clone(),
                package_root: tree_path.clone(),
            },
            cfg.dependencies["base"].dialect.unwrap(),
            &cfg.skills,
            &cfg.dependencies["base"].rename,
            &staging_root,
            &mut diag,
        )
        .unwrap()
    };

    let mut graph = {
        let (mut g, _) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
        g.nodes.get_mut("base").unwrap().rooted_ref = stage(&config);
        g
    };

    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();

    let apply_sync = |graph: &ResolvedGraph, cfg: &EffectiveConfig, lock: &LockFile| {
        let (target, _, _) = target::build_with_collisions(graph, cfg).unwrap();
        let sync_diff = diff::compute(fixture.managed_root(), lock, &target, false).unwrap();
        let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
        let result =
            apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
        let new_lock =
            crate::lock::build(graph, &result, lock, std::collections::BTreeMap::new()).unwrap();
        (sync_diff, sync_plan, new_lock)
    };

    let lock = LockFile::empty();
    let (first_diff, _, first_lock) = apply_sync(&graph, &config, &lock);
    assert!(
        first_diff
            .items
            .iter()
            .all(|entry| matches!(entry, diff::DiffEntry::Add { .. }))
    );

    let (unchanged_diff, unchanged_plan, _) = apply_sync(&graph, &config, &first_lock);
    assert!(
        unchanged_diff
            .items
            .iter()
            .all(|entry| matches!(entry, diff::DiffEntry::Unchanged { .. }))
    );
    assert!(
        unchanged_plan
            .actions
            .iter()
            .all(|action| matches!(action, plan::PlannedAction::Skip { .. }))
    );

    config.skills.insert(
        "planning".to_string(),
        SkillOverlay {
            description: Some("Overridden".to_string()),
            ..SkillOverlay::default()
        },
    );
    graph.nodes.get_mut("base").unwrap().rooted_ref = stage(&config);
    let (overlay_diff, _, overlay_lock) = apply_sync(&graph, &config, &first_lock);
    assert!(
        overlay_diff
            .items
            .iter()
            .any(|entry| matches!(entry, diff::DiffEntry::Update { .. })),
        "expected Update after overlay change, got {:?}",
        overlay_diff.items
    );

    config.dependencies.get_mut("base").unwrap().dialect =
        Some(crate::dialect::Dialect::MarsNative);
    config.skills.clear();
    graph.nodes.get_mut("base").unwrap().rooted_ref = stage(&config);
    let (dialect_diff, _, dialect_lock) = apply_sync(&graph, &config, &overlay_lock);
    assert!(
        dialect_diff
            .items
            .iter()
            .any(|entry| matches!(entry, diff::DiffEntry::Update { .. })),
        "expected Update after dialect change, got {:?}",
        dialect_diff.items
    );

    let (_, frozen_plan, _) = apply_sync(&graph, &config, &dialect_lock);
    assert!(
        frozen_plan.actions.iter().all(|action| {
            matches!(
                action,
                plan::PlannedAction::Skip { .. } | plan::PlannedAction::KeepLocal { .. }
            )
        }),
        "frozen-equivalent re-run should not schedule installs or removals"
    );
}

#[test]
fn validate_skill_refs_ignores_stale_installed_agent_content() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("design-lead.md", "# Design Lead\n")], &[]);
    fs::create_dir_all(fixture.managed_root().join("agents")).unwrap();
    fs::write(
        fixture.managed_root().join("agents/design-lead.md"),
        "---\nskills: [handoff]\n---\n# Stale Design Lead\n",
    )
    .unwrap();

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();

    let warnings = validate::validate_skill_refs(&target);

    assert!(
        warnings.is_empty(),
        "target source removed the missing ref, but stale installed content produced {warnings:?}"
    );
}

#[test]
fn validate_skill_refs_warns_for_missing_target_source_ref() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(
        &[("coder.md", "---\nskills: [missing-skill]\n---\n# Coder\n")],
        &[],
    );

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();

    let warnings = validate::validate_skill_refs(&target);

    assert_eq!(warnings.len(), 1);
    match &warnings[0] {
        ValidationWarning::MissingSkill {
            agent,
            skill_name,
            suggestion,
        } => {
            assert_eq!(agent.name, "coder");
            assert_eq!(skill_name, "missing-skill");
            assert_eq!(suggestion, &None);
        }
    }
}

#[test]
fn validate_skill_refs_uses_rewritten_content() {
    let fixture = TestFixture::new();
    let source_path = fixture.project_root().join("source-agent.md");
    fs::write(
        &source_path,
        "---\nskills: [old-skill]\n---\n# Source content before rewrite\n",
    )
    .unwrap();
    let skill_path = fixture.project_root().join("skills").join("new-skill");
    fs::create_dir_all(&skill_path).unwrap();
    fs::write(skill_path.join("SKILL.md"), "# New Skill\n").unwrap();

    let source_name = SourceName::from("base");
    let source_id = SourceId::Path {
        canonical: fixture.project_root().to_path_buf(),
        subpath: None,
    };
    let mut items = IndexMap::new();
    items.insert(
        DestPath::new("agents/coder.md").unwrap(),
        TargetItem {
            id: ItemId {
                kind: ItemKind::Agent,
                name: "coder".into(),
            },
            source_name: source_name.clone(),
            origin: SourceOrigin::Dependency(source_name.clone()),
            source_id: source_id.clone(),
            source_path,
            dest_path: DestPath::new("agents/coder.md").unwrap(),
            source_hash: ContentHash::from("sha256:source"),
            is_flat_skill: false,
            rewritten_content: Some(
                "---\nskills: [new-skill]\n---\n# Rewritten content\n".to_string(),
            ),
        },
    );
    items.insert(
        DestPath::new("skills/new-skill").unwrap(),
        TargetItem {
            id: ItemId {
                kind: ItemKind::Skill,
                name: "new-skill".into(),
            },
            source_name: source_name.clone(),
            origin: SourceOrigin::Dependency(source_name),
            source_id,
            source_path: skill_path,
            dest_path: DestPath::new("skills/new-skill").unwrap(),
            source_hash: ContentHash::from("sha256:skill"),
            is_flat_skill: false,
            rewritten_content: None,
        },
    );
    let target = TargetState { items };

    let warnings = validate::validate_skill_refs(&target);

    assert!(
        warnings.is_empty(),
        "validation should use rewritten content instead of stale source content: {warnings:?}"
    );
}

#[test]
fn source_update_detects_changes() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Version 1")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    // Update source content
    let agents_dir = fixture.tree_path(src_idx).join("agents");
    fs::write(agents_dir.join("coder.md"), "# Version 2").unwrap();

    // Rebuild target with updated content
    let (target2, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    // Should detect an Update
    assert_eq!(sync_diff2.items.len(), 1);
    assert!(matches!(
        &sync_diff2.items[0],
        diff::DiffEntry::Update { .. }
    ));
}

#[test]
fn local_modification_preserved() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Original")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    // Locally modify the installed file
    fs::write(
        fixture.managed_root().join("agents/coder.md"),
        "# Locally modified",
    )
    .unwrap();

    // Re-sync (source unchanged)
    let (target2, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    // Should detect LocalModified
    assert_eq!(sync_diff2.items.len(), 1);
    assert!(matches!(
        &sync_diff2.items[0],
        diff::DiffEntry::LocalModified { .. }
    ));

    // Plan should KeepLocal
    let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
    assert!(matches!(
        &sync_plan2.actions[0],
        plan::PlannedAction::KeepLocal { .. }
    ));
}

#[test]
fn force_overwrites_local_modifications() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Original")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    // Locally modify the installed file
    fs::write(
        fixture.managed_root().join("agents/coder.md"),
        "# Locally modified",
    )
    .unwrap();

    // Update source too (triggers conflict)
    let agents_dir = fixture.tree_path(src_idx).join("agents");
    fs::write(agents_dir.join("coder.md"), "# Upstream update").unwrap();

    // Re-sync with --force
    let (target2, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    let force_options = SyncOptions {
        force: true,
        ..SyncOptions::default()
    };
    let sync_plan2 = create_sync_plan(&sync_diff2, &force_options, &cache_dir);
    assert!(matches!(
        &sync_plan2.actions[0],
        plan::PlannedAction::Overwrite { .. }
    ));

    let result2 = apply::execute(
        fixture.managed_root(),
        &sync_plan2,
        &force_options,
        &cache_dir,
    )
    .unwrap();
    assert!(matches!(
        result2.outcomes[0].action,
        apply::ActionTaken::Updated
    ));

    // File should have upstream content
    let content = fs::read_to_string(fixture.managed_root().join("agents/coder.md")).unwrap();
    assert_eq!(content, "# Upstream update");
}

#[test]
fn orphan_removed_when_source_drops_item() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(
        &[("coder.md", "# Coder"), ("reviewer.md", "# Reviewer")],
        &[],
    );

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync — install both
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    assert!(fixture.managed_root().join("agents/coder.md").exists());
    assert!(fixture.managed_root().join("agents/reviewer.md").exists());

    // Remove reviewer from source
    fs::remove_file(fixture.tree_path(src_idx).join("agents/reviewer.md")).unwrap();

    // Re-sync
    let (target2, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    // Should have one Unchanged and one Orphan
    let orphan_count = sync_diff2
        .items
        .iter()
        .filter(|e| matches!(e, diff::DiffEntry::Orphan { .. }))
        .count();
    assert_eq!(orphan_count, 1);

    let sync_plan2 = create_sync_plan(&sync_diff2, &options, &cache_dir);
    let result2 =
        apply::execute(fixture.managed_root(), &sync_plan2, &options, &cache_dir).unwrap();

    // Reviewer should be removed
    assert!(!fixture.managed_root().join("agents/reviewer.md").exists());
    // Coder should still be there
    assert!(fixture.managed_root().join("agents/coder.md").exists());

    // Check remove outcome
    let removed = result2
        .outcomes
        .iter()
        .any(|o| matches!(o.action, apply::ActionTaken::Removed));
    assert!(removed);
}

#[test]
fn dry_run_produces_plan_without_changes() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Coder")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();

    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let dry_options = SyncOptions {
        dry_run: true,
        ..SyncOptions::default()
    };

    let sync_plan = create_sync_plan(&sync_diff, &dry_options, &cache_dir);
    assert!(!sync_plan.actions.is_empty());

    // Execute in dry-run mode
    let result =
        apply::execute(fixture.managed_root(), &sync_plan, &dry_options, &cache_dir).unwrap();
    assert!(!result.outcomes.is_empty());

    // No files should have been created
    assert!(!fixture.managed_root().join("agents/coder.md").exists());
}

#[test]
fn lock_written_after_apply() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Coder")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // Full pipeline minus actual sync() (which needs real config files)
    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();

    let new_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();
    crate::lock::write(fixture.project_root(), &new_lock).unwrap();

    // Verify lock file exists and is valid
    let reloaded = crate::lock::load(fixture.project_root()).unwrap();
    assert_eq!(reloaded.items.len(), 1);
    assert!(reloaded.items.contains_key("agent/coder"));

    let item = &reloaded.items["agent/coder"];
    assert_eq!(item.kind, ItemKind::Agent);
    assert!(!item.source_checksum.is_empty());
    assert!(!item.outputs[0].installed_checksum.is_empty());
}

#[test]
fn two_sources_no_collision() {
    let mut fixture = TestFixture::new();
    let src_a = fixture.add_source(&[("coder.md", "# Coder from A")], &[]);
    let src_b = fixture.add_source(&[("reviewer.md", "# Reviewer from B")], &[]);

    let (graph, config) = make_graph_config(
        &fixture,
        vec![
            ("source-a", src_a, FilterMode::All),
            ("source-b", src_b, FilterMode::All),
        ],
    );

    let (target, renames, _) = target::build_with_collisions(&graph, &config).unwrap();
    assert!(renames.is_empty());
    assert_eq!(target.items.len(), 2);

    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let cache_dir = fixture.project_root().join(".mars/cache/bases");
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options, &cache_dir);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options, &cache_dir).unwrap();

    assert!(fixture.managed_root().join("agents/coder.md").exists());
    assert!(fixture.managed_root().join("agents/reviewer.md").exists());
    assert_eq!(result.outcomes.len(), 2);
}

// === Tests for OnlySkills / OnlyAgents filter in pipeline ===

#[test]
fn pipeline_only_skills_filter() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(
        &[("coder.md", "# Coder agent")],
        &[("planning", "# Planning skill")],
    );

    let (graph, config) =
        make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlySkills)]);

    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    // Should only have the skill, not the agent
    assert_eq!(target.items.len(), 1);
    assert!(target.items.contains_key("skills/planning"));
}

#[test]
fn pipeline_only_agents_filter() {
    let mut fixture = TestFixture::new();
    // Agent with a skill dependency in frontmatter
    let agent_content = "---\nskills:\n  - planning\n---\n# Coder agent";
    let src_idx = fixture.add_source(
        &[("coder.md", agent_content)],
        &[
            ("planning", "# Planning skill"),
            ("standalone", "# Standalone skill"),
        ],
    );

    let (graph, config) =
        make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlyAgents)]);

    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    // Should have the agent + its transitive skill dep, but NOT standalone
    assert_eq!(target.items.len(), 2);
    assert!(target.items.contains_key("agents/coder.md"));
    assert!(target.items.contains_key("skills/planning"));
    assert!(!target.items.contains_key("skills/standalone"));
}

#[test]
fn pipeline_only_agents_no_agents_source() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[], &[("planning", "# Planning skill")]);

    let (graph, config) =
        make_graph_config(&fixture, vec![("base", src_idx, FilterMode::OnlyAgents)]);

    let (target, _, _) = target::build_with_collisions(&graph, &config).unwrap();
    // No agents means nothing gets installed
    assert_eq!(target.items.len(), 0);
}
