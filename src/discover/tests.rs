//! Tests for filesystem discovery rules and installed item metadata parsing.
//!
//! These cases pin the package-layer grounding contract, manifest declaration
//! union/collision behavior, and installed frontmatter extraction that resolve
//! and sync depend on.

use super::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn conventional_discovery_finds_agents_and_skills() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(dir.path().join("agents/coder.md"), "# coder").unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

    let items = discover_source(dir.path(), None).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|item| item.source_path == Path::new("agents/coder.md"))
    );
    assert!(
        items
            .iter()
            .any(|item| item.source_path == Path::new("skills/planning"))
    );
}

#[test]
fn conventional_discovery_grounds_to_shallowest_convention_layer() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::create_dir_all(dir.path().join("sub/agents")).unwrap();
    fs::create_dir_all(dir.path().join("a/b/skills/bar")).unwrap();
    fs::write(dir.path().join("agents/top.md"), "# top").unwrap();
    fs::write(dir.path().join("sub/agents/foo.md"), "# foo").unwrap();
    fs::write(dir.path().join("a/b/skills/bar/SKILL.md"), "# bar").unwrap();

    let items = discover_source(dir.path(), None).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("agents/top.md"));
    assert!(
        !items
            .iter()
            .any(|item| item.source_path == Path::new("sub/agents/foo.md"))
    );
    assert!(
        !items
            .iter()
            .any(|item| item.source_path == Path::new("a/b/skills/bar"))
    );
}

#[test]
fn grounding_uses_registering_skill_container_layer_not_first_skill_segment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("outer/skills/foo/skills/bar")).unwrap();
    fs::write(dir.path().join("outer/skills/foo/SKILL.md"), "# foo").unwrap();
    fs::write(
        dir.path().join("outer/skills/foo/skills/bar/SKILL.md"),
        "# bar",
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::Skill);
    assert_eq!(items[0].id.name.as_str(), "foo");
    assert_eq!(items[0].source_path, PathBuf::from("outer/skills/foo"));
}

#[test]
fn grounding_uses_registering_agent_container_layer_not_first_agent_segment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("outer/agents/foo/agents")).unwrap();
    fs::write(dir.path().join("outer/agents/foo.md"), "# foo").unwrap();
    fs::write(dir.path().join("outer/agents/foo/agents/bar.md"), "# bar").unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::Agent);
    assert_eq!(items[0].id.name.as_str(), "foo");
    assert_eq!(items[0].source_path, PathBuf::from("outer/agents/foo.md"));
}

#[test]
fn grounding_uses_registering_bootstrap_container_layer_not_first_bootstrap_segment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("outer/bootstrap/foo/bootstrap/bar")).unwrap();
    fs::write(dir.path().join("outer/bootstrap/foo/BOOTSTRAP.md"), "# foo").unwrap();
    fs::write(
        dir.path()
            .join("outer/bootstrap/foo/bootstrap/bar/BOOTSTRAP.md"),
        "# bar",
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "foo");
    assert_eq!(items[0].source_path, PathBuf::from("outer/bootstrap/foo"));
}

#[test]
fn conventional_discovery_keeps_nested_only_min_layer() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("sub/agents")).unwrap();
    fs::write(dir.path().join("sub/agents/x.md"), "# x").unwrap();

    let items = discover_source(dir.path(), None).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::Agent);
    assert_eq!(items[0].id.name.as_str(), "x");
    assert_eq!(items[0].source_path, PathBuf::from("sub/agents/x.md"));
}

#[test]
fn top_level_skills_suppress_nested_example_skills() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("skills/foo")).unwrap();
    fs::create_dir_all(dir.path().join("examples/skills/bar")).unwrap();
    fs::write(dir.path().join("skills/foo/SKILL.md"), "# foo").unwrap();
    fs::write(dir.path().join("examples/skills/bar/SKILL.md"), "# bar").unwrap();

    let items = discover_source(dir.path(), None).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::Skill);
    assert_eq!(items[0].id.name.as_str(), "foo");
    assert_eq!(items[0].source_path, PathBuf::from("skills/foo"));
    assert!(
        !items
            .iter()
            .any(|item| item.source_path == Path::new("examples/skills/bar"))
    );
}

#[test]
fn conventional_discovery_finds_package_bootstrap_docs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/global-auth")).unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/.hidden")).unwrap();
    fs::write(
        dir.path().join("bootstrap/global-auth/BOOTSTRAP.md"),
        "# auth",
    )
    .unwrap();
    fs::write(dir.path().join("bootstrap/.hidden/BOOTSTRAP.md"), "# hide").unwrap();

    let items = discover_source(dir.path(), None).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "global-auth");
    assert_eq!(items[0].source_path, PathBuf::from("bootstrap/global-auth"));
}

#[test]
fn conventional_bootstrap_discovery_ignores_missing_bootstrap_file() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/incomplete")).unwrap();
    fs::write(
        dir.path().join("bootstrap/incomplete/README.md"),
        "# readme",
    )
    .unwrap();

    let items = discover_source(dir.path(), None).unwrap();
    assert!(items.is_empty());
}

#[test]
fn dispatcher_prefers_conventional_when_manifest_exists() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mars.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();
    fs::create_dir_all(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/SKILL.md"), "# nested").unwrap();

    let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
}

#[test]
fn fallback_root_skill_does_not_override_convention_items() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.name.as_str(), "planning");
    assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
}

#[test]
fn fallback_flat_root_skill_uses_package_basename_when_no_source_name() {
    let dir = TempDir::new().unwrap();
    let pkg = dir.path().join("my-pkg");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(pkg.join("SKILL.md"), "# flat").unwrap();

    let items = discover_manifestless_source(&pkg, None).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.name.as_str(), "my-pkg");
    assert_ne!(items[0].id.name.as_str(), "unknown-source");
}

#[test]
fn fallback_flat_root_skill_uses_source_name_not_staged_dialect_dir() {
    let dir = TempDir::new().unwrap();
    // Simulates staged package root named after inbound dialect (e.g. codex/).
    let staged = dir.path().join("codex");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("SKILL.md"), "# flat foreign skill").unwrap();

    let items = discover_manifestless_source(&staged, Some("base")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.name.as_str(), "base");
    assert_eq!(items[0].source_path, PathBuf::from("."));
}

#[test]
fn fallback_flat_root_skill_overlay_applies_under_source_name() {
    use crate::config::SkillOverlay;
    use crate::diagnostic::DiagnosticCollector;
    use crate::dialect::Dialect;
    use crate::staging::stage_canonical_source;
    use crate::types::RenameMap;
    use indexmap::IndexMap;

    let source = TempDir::new().unwrap();
    fs::write(
        source.path().join("SKILL.md"),
        "---\nname: base\ndescription: base desc\n---\n# Flat\n",
    )
    .unwrap();

    let mut overrides = IndexMap::new();
    overrides.insert(
        "base".to_string(),
        SkillOverlay {
            description: Some("overlay desc".to_string()),
            ..SkillOverlay::default()
        },
    );

    let staged = TempDir::new().unwrap();
    let staged_root = staged.path().join("codex");
    let mut diag = DiagnosticCollector::new();
    stage_canonical_source(
        source.path(),
        &staged_root,
        Dialect::Codex,
        &overrides,
        &RenameMap::new(),
        Some("base"),
        &mut diag,
    )
    .unwrap();

    let items = discover_manifestless_source(&staged_root, Some("base")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.name.as_str(), "base");

    let content = fs::read_to_string(staged_root.join("SKILL.md")).unwrap();
    assert!(
        content.contains("description: overlay desc"),
        "skills.base overlay must apply during staging: {content}"
    );
}

#[test]
fn fallback_flat_root_skill_overlay_applies_after_rename() {
    use crate::config::SkillOverlay;
    use crate::diagnostic::DiagnosticCollector;
    use crate::dialect::Dialect;
    use crate::staging::stage_canonical_source;
    use crate::types::{ItemName, RenameMap};
    use indexmap::IndexMap;

    let source = TempDir::new().unwrap();
    fs::write(
        source.path().join("SKILL.md"),
        "---\nname: base\ndescription: base desc\n---\n# Flat\n",
    )
    .unwrap();

    let mut overrides = IndexMap::new();
    overrides.insert(
        "renamed-skill".to_string(),
        SkillOverlay {
            description: Some("renamed overlay".to_string()),
            ..SkillOverlay::default()
        },
    );
    let mut renames = RenameMap::new();
    renames.insert(ItemName::from("base"), ItemName::from("renamed-skill"));

    let staged = TempDir::new().unwrap();
    let staged_root = staged.path().join("codex");
    let mut diag = DiagnosticCollector::new();
    stage_canonical_source(
        source.path(),
        &staged_root,
        Dialect::Codex,
        &overrides,
        &renames,
        Some("base"),
        &mut diag,
    )
    .unwrap();

    let items = discover_manifestless_source(&staged_root, Some("base")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].id.name.as_str(),
        "base",
        "discovery keys flat skills by source name; rename applies later in target build"
    );

    let content = fs::read_to_string(staged_root.join("SKILL.md")).unwrap();
    assert!(
        content.contains("description: renamed overlay"),
        "skills.renamed-skill overlay must apply after rename during staging: {content}"
    );
}

#[test]
fn fallback_root_skill_includes_manifest_bootstrap_docs() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
    fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
    fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|item| item.id.kind == ItemKind::Skill && item.source_path == Path::new("."))
    );
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::BootstrapDoc && item.source_path == Path::new("docs/global-auth")
    }));
}

#[test]
fn manifestless_source_unions_conventions_with_manifest_declarations() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
    fs::write(dir.path().join("agents/coder.md"), "# coder").unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();
    fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 3);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/coder.md")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/planning")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::BootstrapDoc && item.source_path == Path::new("docs/global-auth")
    }));
}

#[test]
fn manifestless_source_keeps_convention_and_different_named_manifest_item() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("skills/foo")).unwrap();
    fs::create_dir_all(dir.path().join("declared/bar")).unwrap();
    fs::write(dir.path().join("skills/foo/SKILL.md"), "# foo").unwrap();
    fs::write(dir.path().join("declared/bar/SKILL.md"), "# bar").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./declared/bar"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/foo")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill && item.source_path == Path::new("declared/bar")
    }));
}

#[test]
fn manifest_name_collision_with_convention_item_is_reported() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("skills/foo")).unwrap();
    fs::create_dir_all(dir.path().join("declared/foo")).unwrap();
    fs::write(dir.path().join("skills/foo/SKILL.md"), "# convention").unwrap();
    fs::write(dir.path().join("declared/foo/SKILL.md"), "# manifest").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./declared/foo"}]}"#,
    )
    .unwrap();

    let err = discover_manifestless_source(dir.path(), Some("demo")).unwrap_err();

    assert!(matches!(err, MarsError::DiscoveryCollision { .. }));
}

#[test]
fn manifest_name_collision_between_declared_paths_is_reported() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("declared/a/foo")).unwrap();
    fs::create_dir_all(dir.path().join("declared/b/foo")).unwrap();
    fs::write(dir.path().join("declared/a/foo/SKILL.md"), "# a").unwrap();
    fs::write(dir.path().join("declared/b/foo/SKILL.md"), "# b").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./declared/a/foo"},{"path":"./declared/b/foo"}]}"#,
    )
    .unwrap();

    let err = discover_manifestless_source(dir.path(), Some("demo")).unwrap_err();

    assert!(matches!(err, MarsError::DiscoveryCollision { .. }));
}

#[test]
fn duplicate_names_at_same_grounded_layer_are_reported() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a/skills/foo")).unwrap();
    fs::create_dir_all(dir.path().join("b/skills/foo")).unwrap();
    fs::write(dir.path().join("a/skills/foo/SKILL.md"), "# a").unwrap();
    fs::write(dir.path().join("b/skills/foo/SKILL.md"), "# b").unwrap();

    let err = discover_manifestless_source(dir.path(), Some("demo")).unwrap_err();

    assert!(matches!(err, MarsError::DiscoveryCollision { .. }));
}

#[test]
fn manifestless_source_discovers_top_level_canonical_agents_and_skills() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::create_dir_all(dir.path().join("skills/review")).unwrap();
    fs::write(dir.path().join("agents/reviewer.md"), "# reviewer").unwrap();
    fs::write(dir.path().join("skills/review/SKILL.md"), "# review").unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/reviewer.md")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/review")
    }));
}

#[test]
fn convention_walk_finds_items_at_max_depth_and_skips_deeper_items() {
    let dir = TempDir::new().unwrap();
    let at_limit = ["a", "b", "c", "d", AGENTS_DIR_NAME].join("/");
    let beyond_limit = ["a", "b", "c", "d", "e", AGENTS_DIR_NAME].join("/");
    fs::create_dir_all(dir.path().join(&at_limit)).unwrap();
    fs::create_dir_all(dir.path().join(&beyond_limit)).unwrap();
    fs::write(dir.path().join(&at_limit).join("found.md"), "# found").unwrap();
    fs::write(
        dir.path().join(&beyond_limit).join("skipped.md"),
        "# skipped",
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].source_path,
        PathBuf::from("a/b/c/d/agents/found.md")
    );
}

#[test]
fn explicit_claude_subpath_with_claude_dialect_imports_real_claude_layout() {
    use crate::config::SkillOverlay;
    use crate::diagnostic::DiagnosticCollector;
    use crate::dialect::Dialect;
    use crate::resolve::apply_subpath;
    use crate::staging::stage_rooted_source;
    use crate::types::{RenameMap, SourceName, SourceSubpath};
    use indexmap::IndexMap;

    let checkout = TempDir::new().unwrap();
    let claude_root = checkout.path().join(".claude");
    fs::create_dir_all(claude_root.join("agents")).unwrap();
    fs::create_dir_all(claude_root.join("skills/research")).unwrap();
    fs::write(
        claude_root.join("agents/reviewer.md"),
        "---\ndescription: reviewer\n---\n# reviewer",
    )
    .unwrap();
    fs::write(
        claude_root.join("skills/research/SKILL.md"),
        "---\ndescription: research\n---\n# research",
    )
    .unwrap();

    let source_name = SourceName::new("foreign");
    let subpath = SourceSubpath::new(".claude").unwrap();
    let rooted = apply_subpath(&source_name, checkout.path(), Some(&subpath)).unwrap();
    let staging = TempDir::new().unwrap();
    let mut diag = DiagnosticCollector::new();
    let staged = stage_rooted_source(
        &source_name,
        rooted,
        Dialect::Claude,
        &IndexMap::<String, SkillOverlay>::new(),
        &RenameMap::new(),
        staging.path(),
        &mut diag,
    )
    .unwrap();

    let items = discover_resolved_source(&staged.package_root, Some("foreign")).unwrap();

    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Agent && item.source_path == Path::new("agents/reviewer.md")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill && item.source_path == Path::new("skills/research")
    }));
}

#[test]
fn fallback_walk_finds_nested_min_layer_and_skips_dot_dirs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("sub/agents")).unwrap();
    fs::create_dir_all(dir.path().join("a/b/skills/bar")).unwrap();
    fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
    fs::write(dir.path().join("sub/agents/foo.md"), "# agent").unwrap();
    fs::write(dir.path().join("a/b/skills/bar/SKILL.md"), "# skill").unwrap();
    fs::write(dir.path().join(".claude/agents/hidden.md"), "# hidden").unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Agent && item.source_path == Path::new("sub/agents/foo.md")
    }));
    assert!(
        !items
            .iter()
            .any(|item| item.source_path == Path::new("a/b/skills/bar"))
    );
    assert!(
        !items
            .iter()
            .any(|item| item.source_path == Path::new(".claude/agents/hidden.md"))
    );
}

#[test]
fn explicit_claude_subpath_root_imports_inner_convention_dirs() {
    let dir = TempDir::new().unwrap();
    let claude_root = dir.path().join(".claude");
    fs::create_dir_all(claude_root.join("agents")).unwrap();
    fs::create_dir_all(claude_root.join("skills/research")).unwrap();
    fs::write(claude_root.join("agents/reviewer.md"), "# reviewer").unwrap();
    fs::write(claude_root.join("skills/research/SKILL.md"), "# research").unwrap();

    let items = discover_manifestless_source(&claude_root, Some("foreign")).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .any(|item| item.source_path == Path::new("agents/reviewer.md"))
    );
    assert!(
        items
            .iter()
            .any(|item| item.source_path == Path::new("skills/research"))
    );
}

#[test]
fn conventional_root_skill_does_not_override_conventional_items() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mars.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# planning").unwrap();

    let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
}

#[test]
fn conventional_root_skill_survives_bootstrap_only_discovery() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("mars.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(dir.path().join("SKILL.md"), "# root").unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/global-auth")).unwrap();
    fs::write(
        dir.path().join("bootstrap/global-auth/BOOTSTRAP.md"),
        "# auth",
    )
    .unwrap();

    let items = discover_resolved_source(dir.path(), Some("demo")).unwrap();

    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::Skill
            && item.id.name.as_str() == "demo"
            && item.source_path == Path::new(".")
    }));
    assert!(items.iter().any(|item| {
        item.id.kind == ItemKind::BootstrapDoc
            && item.id.name.as_str() == "global-auth"
            && item.source_path == Path::new("bootstrap/global-auth")
    }));
}

#[test]
fn manifest_declared_skill_path_is_honored() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("top-level")).unwrap();
    fs::create_dir_all(dir.path().join("plugins/deep-skill")).unwrap();
    fs::write(dir.path().join("top-level/SKILL.md"), "# top").unwrap();
    fs::write(dir.path().join("plugins/deep-skill/SKILL.md"), "# deep").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./plugins/deep-skill"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("plugins/deep-skill"));
}

#[test]
fn fallback_dedupes_overlapping_manifest_and_container_paths() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(dir.path().join("skills/planning/SKILL.md"), "# skill").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./skills/planning"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("skills/planning"));
}

#[test]
fn manifest_ignores_nested_metadata_agent_keys() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::write(dir.path().join("agents/reviewer.md"), "# reviewer").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"agents":[{"path":"./agents/reviewer.md"}],"metadata":{"agents":[{"path":"./ignore.md"}]}}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source_path, PathBuf::from("agents/reviewer.md"));
}

#[test]
fn fallback_manifest_declares_bootstrap_docs() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("docs/global-auth")).unwrap();
    fs::write(dir.path().join("docs/global-auth/BOOTSTRAP.md"), "# auth").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrapDocs":[{"path":"./docs/global-auth"}]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "global-auth");
    assert_eq!(items[0].source_path, PathBuf::from("docs/global-auth"));
}

#[test]
fn fallback_manifest_declares_bootstrap_container() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("bootstrap/setup")).unwrap();
    fs::write(dir.path().join("bootstrap/setup/BOOTSTRAP.md"), "# setup").unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"bootstrap_docs":["./bootstrap"]}"#,
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "setup");
    assert_eq!(items[0].source_path, PathBuf::from("bootstrap/setup"));
}

#[test]
fn nested_bootstrap_dir_is_discovered() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("nested/bootstrap/setup")).unwrap();
    fs::create_dir_all(dir.path().join("nested/bootstrap/.hidden")).unwrap();
    fs::write(
        dir.path().join("nested/bootstrap/setup/BOOTSTRAP.md"),
        "# setup",
    )
    .unwrap();
    fs::write(
        dir.path().join("nested/bootstrap/.hidden/BOOTSTRAP.md"),
        "# hidden",
    )
    .unwrap();

    let items = discover_manifestless_source(dir.path(), Some("demo")).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.kind, ItemKind::BootstrapDoc);
    assert_eq!(items[0].id.name.as_str(), "setup");
    assert_eq!(
        items[0].source_path,
        PathBuf::from("nested/bootstrap/setup")
    );
}

#[test]
fn fallback_manifest_declared_escape_is_rejected() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        r#"{"skills":[{"path":"./../escape"}]}"#,
    )
    .unwrap();

    let err = discover_manifestless_source(dir.path(), Some("demo")).unwrap_err();
    assert!(matches!(err, MarsError::ManifestDeclaredPathEscape { .. }));
}

#[test]
fn discover_installed_reads_frontmatter() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("agents")).unwrap();
    fs::create_dir_all(dir.path().join("skills/planning")).unwrap();
    fs::write(
        dir.path().join("agents/coder.md"),
        "---\nname: coder\ndescription: test\nskills: [planning]\n---\n# Coder\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("skills/planning/SKILL.md"),
        "---\nname: planning\ndescription: test\n---\n# Planning\n",
    )
    .unwrap();

    let state = discover_installed(dir.path()).unwrap();
    assert_eq!(state.agents.len(), 1);
    assert_eq!(state.skills.len(), 1);
    assert_eq!(state.agents[0].skill_refs, vec!["planning"]);
    assert_eq!(
        state.skills[0].frontmatter_name.as_deref(),
        Some("planning")
    );
}
