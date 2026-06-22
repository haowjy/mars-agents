//! Staging seam integration tests.

use super::*;
use crate::config::{AgentOverlayTools, EffectiveDependency, FilterMode, SkillOverlay};
use crate::dialect::Dialect;
use crate::hash;
use crate::lock::ItemKind;
use crate::types::{RenameMap, SourceId};
use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn path_config_with_dialect(
    name: &str,
    tree: PathBuf,
    dialect: Option<Dialect>,
) -> EffectiveConfig {
    path_config_with_dialect_and_skills(name, tree, dialect, IndexMap::new())
}

fn path_config_with_dialect_and_skills(
    name: &str,
    tree: PathBuf,
    dialect: Option<Dialect>,
    skills: IndexMap<String, SkillOverlay>,
) -> EffectiveConfig {
    EffectiveConfig {
        dependencies: IndexMap::from([(
            name.into(),
            EffectiveDependency {
                name: name.into(),
                id: SourceId::Path {
                    canonical: tree.clone(),
                    subpath: None,
                },
                spec: SourceSpec::Path(tree),
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                dialect,
                is_overridden: false,
                original_git: None,
            },
        )]),
        settings: Settings::default(),
        skills,
    }
}

fn staging_options(dir: &TempDir) -> ResolveOptions {
    ResolveOptions::default().with_staging_root(dir.path().join("staging"))
}

#[test]
fn mars_native_resync_uses_unchanged_staged_hash() {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join("source");
    let skill = source.join("skills/planning");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: planning\n---\n# Planning\n",
    )
    .unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("base", source.clone(), None);
    let config = path_config_with_dialect("base", source.clone(), Some(Dialect::MarsNative));
    let options = staging_options(&tmp);

    let graph1 = resolve(&config, &provider, None, &options).unwrap();
    let graph2 = resolve(&config, &provider, None, &options).unwrap();

    let staged = &graph1.nodes["base"].rooted_ref.package_root;
    assert!(staged.starts_with(tmp.path().join("staging")));
    assert_eq!(
        graph1.nodes["base"].rooted_ref.package_root,
        graph2.nodes["base"].rooted_ref.package_root
    );

    let hash1 = hash::compute_hash(&staged.join("skills/planning"), ItemKind::Skill).unwrap();
    let hash2 = hash::compute_hash(
        &graph2.nodes["base"]
            .rooted_ref
            .package_root
            .join("skills/planning"),
        ItemKind::Skill,
    )
    .unwrap();
    assert_eq!(hash1, hash2);
}

#[test]
fn explicit_dialect_change_flips_staged_skill_hash() {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join("source");
    let skill = source.join("skills/planning");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: planning\ndescription: d\ndisable-model-invocation: true\n---\n# Planning\n",
    )
    .unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("base", source.clone(), None);
    let options = staging_options(&tmp);

    let native_config = path_config_with_dialect("base", source.clone(), Some(Dialect::MarsNative));
    let claude_config = path_config_with_dialect("base", source, Some(Dialect::Claude));

    let native_graph = resolve(&native_config, &provider, None, &options).unwrap();
    let claude_graph = resolve(&claude_config, &provider, None, &options).unwrap();

    let native_staged = native_graph.nodes["base"].rooted_ref.package_root.clone();
    let claude_staged = claude_graph.nodes["base"].rooted_ref.package_root.clone();
    assert_ne!(native_staged, claude_staged);

    let native_hash =
        hash::compute_hash(&native_staged.join("skills/planning"), ItemKind::Skill).unwrap();
    let claude_hash =
        hash::compute_hash(&claude_staged.join("skills/planning"), ItemKind::Skill).unwrap();
    assert_ne!(native_hash, claude_hash);
}

#[test]
fn skill_overlay_change_flips_staged_hash_and_removal_restores() {
    let tmp = TempDir::new().unwrap();
    let source = tmp.path().join("source");
    let skill = source.join("skills/planning");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: planning\ndescription: base\nuser-invocable: true\n---\n# Planning\n",
    )
    .unwrap();

    let mut provider = MockProvider::new();
    provider.add_source("base", source.clone(), None);
    let options = staging_options(&tmp);

    let no_overlay =
        path_config_with_dialect_and_skills("base", source.clone(), Some(Dialect::Claude), IndexMap::new());
    let baseline_graph = resolve(&no_overlay, &provider, None, &options).unwrap();
    let baseline_hash = hash::compute_hash(
        &baseline_graph.nodes["base"]
            .rooted_ref
            .package_root
            .join("skills/planning"),
        ItemKind::Skill,
    )
    .unwrap();

    let mut overrides = IndexMap::new();
    overrides.insert(
        "planning".to_string(),
        SkillOverlay {
            description: Some("Overridden".to_string()),
            user_invocable: Some(false),
            ..SkillOverlay::default()
        },
    );
    let with_overlay = path_config_with_dialect_and_skills(
        "base",
        source.clone(),
        Some(Dialect::Claude),
        overrides,
    );
    let overlay_graph = resolve(&with_overlay, &provider, None, &options).unwrap();
    let overlay_hash = hash::compute_hash(
        &overlay_graph.nodes["base"]
            .rooted_ref
            .package_root
            .join("skills/planning"),
        ItemKind::Skill,
    )
    .unwrap();
    assert_ne!(baseline_hash, overlay_hash);

    let restored_graph = resolve(&no_overlay, &provider, None, &options).unwrap();
    let restored_hash = hash::compute_hash(
        &restored_graph.nodes["base"]
            .rooted_ref
            .package_root
            .join("skills/planning"),
        ItemKind::Skill,
    )
    .unwrap();
    assert_eq!(baseline_hash, restored_hash);
}
