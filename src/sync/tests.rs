//! Tests for the sync pipeline: target building, validation, planning, and apply.

use super::*;
use crate::config::*;
use crate::lock::LockFile;
use crate::resolve::{ResolvedGraph, ResolvedNode};
use indexmap::IndexMap;
use mars_schema::dependency::Entry;
use mars_schema::filter::{FilterConfig, FilterMode};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// The consumer's own package in a dependency-only test graph.
fn test_local_source() -> crate::types::SourceName {
    crate::types::SourceOrigin::LocalPackage.to_string().into()
}

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
                fetch_url: None,
                source_name: name.into(),
                source_id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                    plugin: None,
                },
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: tree_path.clone(),
                    authored_root: tree_path.clone(),
                    package_root: tree_path.clone(),
                    plugin: None,
                },
                prepared: crate::package_prepare::prepare_tree_for_test(name, &tree_path),
                resolved_ref: crate::source::ResolvedRef {
                    source_name: name.into(),
                    version: None,
                    version_tag: None,
                    commit: None,
                    tree_path: tree_path.clone(),
                },
                manifest: None,
            },
        );
        order.push(name.into());

        config_dependencies.insert(
            name.into(),
            EffectiveDependency {
                declared_source_id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                    plugin: None,
                },
                source_id: crate::types::SourceId::Path {
                    canonical: tree_path.clone(),
                    subpath: None,
                    plugin: None,
                },
                spec: SourceSpec::Path(tree_path),
                subpath: None,
                filter,
                rename: mars_schema::names::RenameMap::new(),
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
    (
        ResolvedGraph {
            selection: crate::resolve::select_for_test(&config, &nodes),
            nodes,
            order,
            version_constraints: std::collections::HashMap::new(),
            unreadable_hook_surfaces: std::collections::BTreeMap::new(),
        },
        config,
    )
}

fn path_dependency_entry(path: &std::path::Path) -> Entry {
    Entry::at_path(path.to_path_buf())
}

fn git_dependency_entry(
    url: &str,
    version: &str,
    filter: FilterConfig,
) -> mars_schema::dependency::RawEntry {
    Entry {
        version: Some(version.to_string()),
        filter,
        ..Entry::git(url)
    }
    .into()
}

fn create_sync_plan(sync_diff: &diff::SyncDiff, options: &SyncOptions) -> plan::SyncPlan {
    let mut diag = DiagnosticCollector::new();
    plan::create(sync_diff, options, &mut diag)
}

#[test]
fn build_target_prunes_unmanaged_collision_before_rewriting_refs() {
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(
        &[("coder.md", "---\nskills: [planning]\n---\n# Agent\n")],
        &[("planning", "# Planning A")],
    );
    let source_b = fixture.add_source(&[], &[("planning", "# Planning B")]);
    fs::write(
        fixture.tree_path(source_a).join("mars.toml"),
        "[package]\nname = \"source-a\"\nversion = \"1.0.0\"\n\n[dependencies.source-b]\npath = \"../source-b\"\n",
    )
    .unwrap();
    let (graph, mut effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
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
            provided: crate::provided::ProvidedNames::default(),
            dependency_changes: Vec::new(),
            sync_lock: Some(sync_lock),
        },
        graph,
        upgrades_available: 0,
        preparation: crate::platform::fs::PendingStagingGeneration::create(
            &fixture.project_root().join(".mars"),
        )
        .unwrap(),
    };
    let ctx = MarsContext::for_test(fixture.project_root().to_path_buf());
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        recovery: Default::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let mut diag = DiagnosticCollector::new();

    let targeted = build_target(&ctx, resolved, Vec::new(), &request, &mut diag).unwrap();

    assert!(!targeted.target.items.contains_key("skills/planning"));
    let rewritten = targeted.target.items["agents/coder.md"]
        .rewritten_content
        .as_ref()
        .expect("agent ref should rewrite after unmanaged source skill is pruned");
    let fm = mars_schema::frontmatter::parse(rewritten).unwrap();
    assert_eq!(fm.skills(), vec!["planning__source-b"]);
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
            provided: crate::provided::ProvidedNames::default(),
            dependency_changes: Vec::new(),
            sync_lock: Some(sync_lock),
        },
        graph,
        upgrades_available: 0,
        preparation: crate::platform::fs::PendingStagingGeneration::create(
            &fixture.project_root().join(".mars"),
        )
        .unwrap(),
    };
    let ctx = MarsContext::for_test(fixture.project_root().to_path_buf());
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        recovery: Default::default(),
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

fn graph_with_versions(entries: &[(&str, &str, &str)]) -> ResolvedGraph {
    let mut nodes = IndexMap::new();
    let mut order = Vec::new();
    for (name, url, tag) in entries {
        let version = semver::Version::parse(tag.trim_start_matches('v')).unwrap();
        nodes.insert(
            (*name).into(),
            ResolvedNode {
                fetch_url: None,
                source_name: (*name).into(),
                source_id: crate::types::SourceId::git_with_subpath(
                    crate::types::SourceUrl::from(*url),
                    None,
                ),
                rooted_ref: crate::resolve::RootedSourceRef {
                    checkout_root: PathBuf::from(format!("/tmp/{name}")),
                    authored_root: PathBuf::from(format!("/tmp/{name}")),
                    package_root: PathBuf::from(format!("/tmp/{name}")),
                    plugin: None,
                },
                prepared: crate::resolve::prepared_with_deps(&[]),
                resolved_ref: crate::source::ResolvedRef {
                    source_name: (*name).into(),
                    version: Some(version),
                    version_tag: Some((*tag).to_string()),
                    commit: Some("abc123".into()),
                    tree_path: PathBuf::from(format!("/tmp/{name}")),
                },
                manifest: None,
            },
        );
        order.push((*name).into());
    }

    ResolvedGraph {
        nodes,
        order,
        selection: Default::default(),
        version_constraints: std::collections::HashMap::new(),
        unreadable_hook_surfaces: std::collections::BTreeMap::new(),
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
        recovery: Default::default(),
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
        recovery: Default::default(),
        lossiness_mode: LossinessMode::Hidden,
    };

    let err = validate_request(&request).unwrap_err();
    assert!(matches!(err, MarsError::InvalidRequest { .. }));
    assert!(err.to_string().contains("cannot modify config"));
}

#[test]
fn planned_bump_entries_preserve_filters_and_renames() {
    let mut rename = mars_schema::names::RenameMap::new();
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
fn execute_dry_run_with_mutation_does_not_write_config() {
    let project_root = TempDir::new().unwrap();
    let managed_root = project_root.path().join(".agents");
    crate::config::save(
        project_root.path(),
        &Config {
            dependencies: Default::default(),
            settings: Settings::default(),
            models: indexmap::IndexMap::new(),
            agents: indexmap::IndexMap::new(),
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
            entry: Box::new(path_dependency_entry(source.path())),
        }),
        options: SyncOptions {
            dry_run: true,
            ..SyncOptions::default()
        },
        recovery: Default::default(),
        lossiness_mode: LossinessMode::Hidden,
    };

    let ctx = MarsContext::for_test(project_root.path().to_path_buf());
    let report = execute(&ctx, &request).unwrap();
    assert!(!report.applied.outcomes.is_empty());

    let saved = crate::config::load(project_root.path()).unwrap();
    assert!(!saved.dependencies.contains_key("base"));
    assert!(!managed_root.join("agents/coder.md").exists());
    assert!(!project_root.path().join("mars.lock").exists());
}

// === Integration tests for the pipeline stages ===

#[test]
fn local_modification_preserved() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Original")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    // First sync
    let (target, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();
    let options = SyncOptions::default();
    let sync_plan = create_sync_plan(&sync_diff, &options);
    let result = apply::execute(fixture.managed_root(), &sync_plan, &options).unwrap();
    let first_lock =
        crate::lock::build(&graph, &result, &lock, std::collections::BTreeMap::new()).unwrap();

    // Locally modify the installed file
    fs::write(
        fixture.managed_root().join("agents/coder.md"),
        "# Locally modified",
    )
    .unwrap();

    // Re-sync (source unchanged)
    let (target2, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    let sync_diff2 = diff::compute(fixture.managed_root(), &first_lock, &target2, false).unwrap();

    // Should detect LocalModified
    assert_eq!(sync_diff2.items.len(), 1);
    assert!(matches!(
        &sync_diff2.items[0],
        diff::DiffEntry::LocalModified { .. }
    ));

    // Plan should KeepLocal
    let sync_plan2 = create_sync_plan(&sync_diff2, &options);
    assert!(matches!(
        &sync_plan2.actions[0],
        plan::PlannedAction::KeepLocal { .. }
    ));
}

#[test]
fn dry_run_produces_plan_without_changes() {
    let mut fixture = TestFixture::new();
    let src_idx = fixture.add_source(&[("coder.md", "# Coder")], &[]);

    let (graph, config) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);

    let (target, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    let lock = LockFile::empty();
    let sync_diff = diff::compute(fixture.managed_root(), &lock, &target, false).unwrap();

    let dry_options = SyncOptions {
        dry_run: true,
        ..SyncOptions::default()
    };

    let sync_plan = create_sync_plan(&sync_diff, &dry_options);
    assert!(!sync_plan.actions.is_empty());

    // Execute in dry-run mode
    let result = apply::execute(fixture.managed_root(), &sync_plan, &dry_options).unwrap();
    assert!(!result.outcomes.is_empty());

    // No files should have been created
    assert!(!fixture.managed_root().join("agents/coder.md").exists());
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

    let (target, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
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

    let (target, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
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

    let (target, _, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();
    // No agents means nothing gets installed
    assert_eq!(target.items.len(), 0);
}
