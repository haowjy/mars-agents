//! Tests for the sync pipeline: target building, validation, planning, and apply.

use super::*;
use crate::config::*;
use crate::lock::{ItemId, LockFile};
use crate::resolve::{ResolvedGraph, ResolvedNode};
use crate::sync::target::TargetItem;
use crate::types::{ContentHash, DestPath};
use indexmap::IndexMap;
use mars_schema::dependency::Entry;
use mars_schema::filter::{FilterConfig, FilterMode};
use mars_schema::item::ItemKind;
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

/// Run `build_target` over an in-memory graph and config, as a normal sync would.
fn build_target_for(
    fixture: &TestFixture,
    graph: ResolvedGraph,
    effective: EffectiveConfig,
    diag: &mut DiagnosticCollector,
) -> TargetedState {
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
    build_target(&ctx, resolved, Vec::new(), &request, diag).unwrap()
}

/// The `config-rename-dangle` warnings a build of `graph` and `effective` raises.
fn dangle_warnings(
    fixture: &TestFixture,
    graph: ResolvedGraph,
    effective: EffectiveConfig,
) -> Vec<String> {
    let mut diag = DiagnosticCollector::new();
    build_target_for(fixture, graph, effective, &mut diag);
    diag.drain()
        .into_iter()
        .filter(|d| d.code == "config-rename-dangle")
        .map(|d| d.message)
        .collect()
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

    let targeted = build_target_for(&fixture, graph, effective, &mut DiagnosticCollector::new());

    assert!(!targeted.target.items.contains_key("skills/planning"));
    let rewritten = targeted.target.items["agents/coder.md"]
        .rewritten_content
        .as_ref()
        .expect("agent ref should rewrite after unmanaged source skill is pruned");
    let fm = mars_schema::frontmatter::parse(rewritten).unwrap();
    assert_eq!(fm.skills(), vec!["planning__source-b"]);
}

#[test]
fn build_target_warns_when_config_references_a_renamed_item() {
    // Fanout entry naming an agent that a collision renamed.
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
    let dangles = dangle_warnings(&fixture, graph, effective);
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

    // Agent overlay naming a collision-renamed agent.
    let mut fixture = TestFixture::new();
    let source_a = fixture.add_source(&[("coder.md", "---\nname: coder\n---\n# A\n")], &[]);
    let source_b = fixture.add_source(&[("coder.md", "---\nname: coder\n---\n# B\n")], &[]);
    let (graph, mut effective) = make_graph_config(
        &fixture,
        vec![
            ("source-a", source_a, FilterMode::All),
            ("source-b", source_b, FilterMode::All),
        ],
    );
    effective.agents.insert(
        "coder".into(),
        crate::config::AgentOverlay {
            model: Some("gpt55".into()),
            ..Default::default()
        },
    );
    let dangles = dangle_warnings(&fixture, graph, effective);
    assert_eq!(dangles.len(), 1, "got: {dangles:?}");
    assert!(dangles[0].contains("coder"), "{}", dangles[0]);
    assert!(dangles[0].contains("[agents.<name>]"), "{}", dangles[0]);

    // Skill overlay naming an explicitly renamed skill.
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
    let dangles = dangle_warnings(&fixture, graph, effective);
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

    let dangles = dangle_warnings(&fixture, graph, effective);
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
fn validate_request_rejects_frozen_with_maximize_or_mutation() {
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
        Entry::git("https://example.com/floating.git").into(),
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
                spec: SourceSpec::Path(tree_path.clone()),
                subpath: None,
                filter: FilterMode::All,
                rename: mars_schema::names::RenameMap::new(),
                dialect: Some(mars_schema::dialect::Dialect::Claude),
            },
        )]),
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    };

    let stage = |cfg: &EffectiveConfig| {
        let mut diag = DiagnosticCollector::new();
        let prepared = crate::package_prepare::prepare_rooted(
            &"base".into(),
            crate::resolve::RootedSourceRef {
                checkout_root: tree_path.clone(),
                authored_root: tree_path.clone(),
                package_root: tree_path.clone(),
                plugin: None,
            },
            cfg,
            &mut diag,
        )
        .unwrap();
        crate::staging::materialize_prepared_rooted(
            &"base".into(),
            prepared.rooted,
            prepared.prepared,
            &cfg.skills,
            &mars_schema::names::RenameMap::new(),
            &staging_root,
            &mut diag,
        )
        .unwrap()
        .0
    };

    let mut graph = {
        let (mut g, _) = make_graph_config(&fixture, vec![("base", src_idx, FilterMode::All)]);
        g.nodes.get_mut("base").unwrap().rooted_ref = stage(&config);
        g
    };

    let options = SyncOptions::default();

    let apply_sync = |graph: &ResolvedGraph, cfg: &EffectiveConfig, lock: &LockFile| {
        let (target, _, _) = target::build_with_collisions_and_diag(
            graph,
            cfg,
            &test_local_source(),
            &mut DiagnosticCollector::new(),
        )
        .unwrap();
        let sync_diff = diff::compute(fixture.managed_root(), lock, &target, false).unwrap();
        let sync_plan = create_sync_plan(&sync_diff, &options);
        let result = apply::execute(fixture.managed_root(), &sync_plan, &options).unwrap();
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
        Some(mars_schema::dialect::Dialect::MarsNative);
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
    let (target, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();

    let warnings = validate::validate_skill_refs(&target, &graph, None, None);

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
    let (target, _, _) = target::build_with_collisions_and_diag(
        &graph,
        &config,
        &test_local_source(),
        &mut DiagnosticCollector::new(),
    )
    .unwrap();

    let warnings = validate::validate_skill_refs(&target, &graph, None, None);

    assert_eq!(warnings.len(), 1);
    let ValidationWarning::MissingSkill {
        agent,
        skill_name,
        suggestion,
    } = &warnings[0]
    else {
        panic!("expected a missing-skill warning, got {:?}", warnings[0]);
    };
    assert_eq!(agent.name, "coder");
    assert_eq!(skill_name, "missing-skill");
    assert_eq!(suggestion, &None);
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
    let mut items = IndexMap::new();
    items.insert(
        DestPath::new("agents/coder.md").unwrap(),
        TargetItem {
            id: ItemId {
                kind: ItemKind::Agent,
                name: "coder".into(),
            },
            source_name: source_name.clone(),
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
            source_path: skill_path,
            dest_path: DestPath::new("skills/new-skill").unwrap(),
            source_hash: ContentHash::from("sha256:skill"),
            is_flat_skill: false,
            rewritten_content: None,
        },
    );
    let target = TargetState::from_items(items);

    let (graph, _) = make_graph_config(&fixture, Vec::new());
    let warnings = validate::validate_skill_refs(&target, &graph, None, None);

    assert!(
        warnings.is_empty(),
        "validation should use rewritten content instead of stale source content: {warnings:?}"
    );
}

// === Tests for OnlySkills / OnlyAgents filter in pipeline ===

#[test]
fn consumer_package_requirement_is_hard_and_ignore_flag_bypasses_it() {
    let fixture = TestFixture::new();
    fs::write(
        fixture.project_root().join("mars.toml"),
        "[package]\nname = \"consumer\"\nversion = \"1.0.0\"\nrequires-mars = \">=99\"\n",
    )
    .unwrap();
    let ctx = MarsContext::for_test(fixture.project_root().to_path_buf());
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions::default(),
        recovery: Default::default(),
        lossiness_mode: LossinessMode::Hidden,
    };
    let error = match load_config(&ctx, &request, &mut DiagnosticCollector::new()) {
        Ok(_) => panic!("incompatible consumer requirement should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("consumer package requires mars"));

    let ignored = SyncRequest {
        options: SyncOptions {
            ignore_requires_mars: true,
            ..SyncOptions::default()
        },
        ..request
    };
    let mut diagnostics = DiagnosticCollector::new();
    load_config(&ctx, &ignored, &mut diagnostics).unwrap();
    assert!(
        diagnostics
            .drain()
            .iter()
            .any(|diagnostic| diagnostic.code == "requires-mars-disabled")
    );
}
