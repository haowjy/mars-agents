//! Canonical source staging — lift foreign frontmatter before discovery/validation.
//!
//! `stage_canonical_source` copies a fetched package tree into a derived,
//! dependency-scoped directory and routes markdown frontmatter through
//! `lift_frontmatter`. Downstream pipeline stages read only the staged tree.

mod lift;
mod overlay;

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::config::SkillOverlay;
use crate::dialect::Dialect;
use crate::error::MarsError;
use crate::frontmatter::Frontmatter;
use crate::lock::ItemKind;
use crate::platform::cache::safe_component;
use crate::resolve::RootedSourceRef;
use crate::types::{RenameMap, SourceName};

pub(crate) use lift::lift_frontmatter_with_change;
pub(crate) use overlay::{apply_skill_overlay, skill_overlay_lookup_name};

/// Stage a package tree and repoint `package_root` at the staged output.
pub(crate) fn stage_rooted_source(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    staging_root: &Path,
) -> Result<RootedSourceRef, MarsError> {
    let staged_package_root = staging_dir_for(staging_root, source_name, dialect);
    stage_canonical_source(
        &rooted.package_root,
        &staged_package_root,
        dialect,
        skill_overrides,
        renames,
        Some(source_name.as_ref()),
    )?;
    Ok(RootedSourceRef {
        checkout_root: rooted.checkout_root,
        package_root: staged_package_root,
    })
}

/// Copy `source_root` into `dest_root`, rewriting frontmatter through lift.
pub(crate) fn stage_canonical_source(
    source_root: &Path,
    dest_root: &Path,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
) -> Result<(), MarsError> {
    if dest_root.exists() {
        fs::remove_dir_all(dest_root)?;
    }
    fs::create_dir_all(dest_root)?;
    copy_and_lift_tree(
        source_root,
        dest_root,
        source_root,
        dialect,
        skill_overrides,
        renames,
        fallback_skill_name,
    )
}

/// Stage a single local item path (agent file or skill directory).
pub(crate) fn stage_local_item(
    source_path: &Path,
    kind: ItemKind,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    staging_root: &Path,
    item_key: &str,
    skill_overlay_key: Option<&str>,
) -> Result<PathBuf, MarsError> {
    let dest = staging_root
        .join("_local")
        .join(safe_component(item_key))
        .join(dialect.as_str());
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(dest.parent().unwrap_or(&dest))?;

    match kind {
        ItemKind::Agent | ItemKind::Hook | ItemKind::McpServer => {
            fs::create_dir_all(&dest)?;
            let dest_file = dest.join(
                source_path
                    .file_name()
                    .ok_or_else(|| MarsError::Source {
                        source_name: "_local".to_string(),
                        message: format!(
                            "local agent path has no file name: {}",
                            source_path.display()
                        ),
                    })?,
            );
            process_markdown_file(
                source_path,
                &dest_file,
                kind,
                &StageOverlayContext {
                    dialect,
                    skill_overrides,
                    renames: &RenameMap::new(),
                    package_root: source_path.parent().unwrap_or(source_path),
                    fallback_skill_name: None,
                    skill_overlay_key,
                },
            )?;
            Ok(dest_file)
        }
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            stage_canonical_source(
                source_path,
                &dest,
                dialect,
                skill_overrides,
                &RenameMap::new(),
                skill_overlay_key,
            )?;
            Ok(dest)
        }
    }
}

fn staging_dir_for(staging_root: &Path, source_name: &SourceName, dialect: Dialect) -> PathBuf {
    staging_root
        .join(safe_component(source_name.as_ref()))
        .join(dialect.as_str())
}

struct StageOverlayContext<'a> {
    dialect: Dialect,
    skill_overrides: &'a IndexMap<String, SkillOverlay>,
    renames: &'a RenameMap,
    package_root: &'a Path,
    fallback_skill_name: Option<&'a str>,
    skill_overlay_key: Option<&'a str>,
}

fn copy_and_lift_tree(
    source_root: &Path,
    dest_root: &Path,
    current: &Path,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
) -> Result<(), MarsError> {
    let mut entries: Vec<_> = fs::read_dir(current)?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let src_path = entry.path();
        let rel = src_path.strip_prefix(source_root).map_err(|_| MarsError::Source {
            source_name: "staging".to_string(),
            message: format!(
                "staging traversal escaped source root at {}",
                src_path.display()
            ),
        })?;
        let dest_path = dest_root.join(rel);
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_and_lift_tree(
                source_root,
                dest_root,
                &src_path,
                dialect,
                skill_overrides,
                renames,
                fallback_skill_name,
            )?;
        } else if should_lift_markdown(&src_path) {
            let kind = item_kind_for_markdown(&src_path);
            process_markdown_file(
                &src_path,
                &dest_path,
                kind,
                &StageOverlayContext {
                    dialect,
                    skill_overrides,
                    renames,
                    package_root: source_root,
                    fallback_skill_name,
                    skill_overlay_key: None,
                },
            )?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn should_lift_markdown(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("SKILL.md") | Some("BOOTSTRAP.md")
    ) || path.extension().is_some_and(|ext| ext == "md" || ext == "mdc")
}

fn item_kind_for_markdown(path: &Path) -> ItemKind {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("SKILL.md") => ItemKind::Skill,
        Some("BOOTSTRAP.md") => ItemKind::BootstrapDoc,
        _ if path.extension().is_some_and(|ext| ext == "mdc") => ItemKind::Skill,
        _ => ItemKind::Agent,
    }
}

fn process_markdown_file(
    src: &Path,
    dest: &Path,
    kind: ItemKind,
    ctx: &StageOverlayContext<'_>,
) -> Result<(), MarsError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let skill_overlay = (kind == ItemKind::Skill)
        .then(|| {
            ctx.skill_overlay_key
                .map(str::to_owned)
                .or_else(|| {
                    skill_overlay_lookup_name(
                        src,
                        ctx.package_root,
                        ctx.renames,
                        ctx.fallback_skill_name,
                    )
                })
        })
        .flatten()
        .and_then(|name| ctx.skill_overrides.get(&name));

    let original = fs::read_to_string(src)?;
    if let Ok(parsed) = Frontmatter::parse(&original) {
        let (mut fm, mut changed) = lift_frontmatter_with_change(ctx.dialect, kind, &parsed);

        if let Some(overlay) = skill_overlay
            && !overlay.is_empty()
        {
            let (overlaid, overlay_changed) = apply_skill_overlay(&fm, overlay);
            fm = overlaid;
            changed |= overlay_changed;
        }

        if changed {
            fs::write(dest, fm.render())?;
        } else {
            fs::copy(src, dest)?;
        }
    } else {
        fs::copy(src, dest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentOverlayTools, SkillOverlay};
    use crate::hash;
    use crate::types::{ItemName, RenameMap};
    use tempfile::TempDir;

    #[test]
    fn stage_skill_overlay_changes_frontmatter_and_preserves_unaffected_bytes() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: base\nuser-invocable: true\n---\n# Body\n",
        )
        .unwrap();
        let other = source.path().join("skills/other");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("SKILL.md"),
            "---\nname: other\ndescription: untouched\n---\n# Other\n",
        )
        .unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert(
            "demo".to_string(),
            SkillOverlay {
                description: Some("Overridden".to_string()),
                user_invocable: Some(false),
                tools: AgentOverlayTools {
                    disallowed: vec!["Agent".to_string()],
                    ..AgentOverlayTools::default()
                },
                ..SkillOverlay::default()
            },
        );

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::Claude,
            &overrides,
            &RenameMap::new(),
            None,
        )
        .unwrap();

        let demo_staged = fs::read_to_string(dest.path().join("skills/demo/SKILL.md")).unwrap();
        assert!(demo_staged.contains("description: Overridden"));
        assert!(demo_staged.contains("user-invocable: false"));
        assert!(demo_staged.contains("disallowed-tools:"));

        let other_staged = fs::read_to_string(dest.path().join("skills/other/SKILL.md")).unwrap();
        assert_eq!(
            other_staged,
            fs::read_to_string(other.join("SKILL.md")).unwrap()
        );
    }

    #[test]
    fn empty_skill_overlay_leaves_bytes_identical() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        let original = "---\nname: demo\ndescription: base\n---\n# Body\n";
        fs::write(skill.join("SKILL.md"), original).unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert("demo".to_string(), SkillOverlay::default());

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::Claude,
            &overrides,
            &RenameMap::new(),
            None,
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(dest.path().join("skills/demo/SKILL.md")).unwrap(),
            original
        );
    }

    #[test]
    fn stage_skill_directory_is_faithful_copy_with_identity_lift() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: demo\n---\n# Body\n").unwrap();
        fs::write(skill.join("helper.sh"), "#!/bin/sh\n").unwrap();

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::Claude,
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        )
        .unwrap();

        let staged_skill = dest.path().join("skills/demo");
        assert!(staged_skill.join("helper.sh").exists());
        assert_eq!(
            fs::read_to_string(staged_skill.join("SKILL.md")).unwrap(),
            "---\nname: demo\n---\n# Body\n"
        );
        assert_eq!(
            hash::compute_hash(&staged_skill, ItemKind::Skill).unwrap(),
            hash::compute_hash(&skill, ItemKind::Skill).unwrap()
        );
    }

    #[test]
    fn claude_foreign_skill_lift_invalidates_staged_hash() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        let native = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            native.path(),
            Dialect::MarsNative,
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        )
        .unwrap();

        let claude = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            claude.path(),
            Dialect::Claude,
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        )
        .unwrap();

        let native_staged = fs::read_to_string(native.path().join("skills/demo/SKILL.md")).unwrap();
        let claude_staged = fs::read_to_string(claude.path().join("skills/demo/SKILL.md")).unwrap();
        assert!(native_staged.contains("disable-model-invocation"));
        assert!(!claude_staged.contains("disable-model-invocation"));
        assert!(claude_staged.contains("model-invocable: false"));

        let native_hash =
            hash::compute_hash(&native.path().join("skills/demo"), ItemKind::Skill).unwrap();
        let claude_hash =
            hash::compute_hash(&claude.path().join("skills/demo"), ItemKind::Skill).unwrap();
        assert_ne!(native_hash, claude_hash);
    }

    #[test]
    fn stage_skill_overlay_keys_by_installed_name_after_rename() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/planning");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: planning\ndescription: base\n---\n# Planning\n",
        )
        .unwrap();

        let mut renames = RenameMap::new();
        renames.insert(ItemName::from("planning"), ItemName::from("research-planning"));

        let mut overrides = IndexMap::new();
        overrides.insert(
            "research-planning".to_string(),
            SkillOverlay {
                description: Some("Renamed overlay".to_string()),
                ..SkillOverlay::default()
            },
        );

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::Claude,
            &overrides,
            &renames,
            None,
        )
        .unwrap();

        let staged = fs::read_to_string(dest.path().join("skills/planning/SKILL.md")).unwrap();
        assert!(staged.contains("description: Renamed overlay"));
    }

    #[test]
    fn stage_flat_skill_overlay_uses_fallback_name() {
        let source = TempDir::new().unwrap();
        fs::write(
            source.path().join("SKILL.md"),
            "---\nname: base\ndescription: base\n---\n# Flat\n",
        )
        .unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert(
            "my-flat-skill".to_string(),
            SkillOverlay {
                description: Some("Flat overlay".to_string()),
                ..SkillOverlay::default()
            },
        );

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::Claude,
            &overrides,
            &RenameMap::new(),
            Some("my-flat-skill"),
        )
        .unwrap();

        let staged = fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert!(staged.contains("description: Flat overlay"));
    }

    #[test]
    fn mars_native_staging_strips_non_canonical_tool_aliases() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/bad-allowed");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: bad-allowed\ndescription: d\nallowed-tools: [Bash]\n---\n# Body\n",
        )
        .unwrap();

        let dest = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            dest.path(),
            Dialect::MarsNative,
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        )
        .unwrap();

        let staged = fs::read_to_string(dest.path().join("skills/bad-allowed/SKILL.md")).unwrap();
        assert!(
            !staged.contains("allowed-tools"),
            "canonical staging must strip non-canonical tool aliases: {staged}"
        );

        let mut diags = Vec::new();
        crate::compiler::skills::parse_skill_content(
            &fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            &mut diags,
        )
        .unwrap();
        assert!(
            diags.iter().any(|d| matches!(
                d,
                crate::compiler::skills::SkillDiagnostic::NonCanonicalField { field, .. }
                    if field == "allowed-tools"
            )),
            "source parse should still emit non-canonical diagnostic: {diags:?}"
        );
    }

    #[test]
    fn local_bare_skill_keeps_foreign_key_for_mars_native_validation() {
        let root = TempDir::new().unwrap();
        let mars_src = root.path().join(".mars-src");
        let skill = mars_src.join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        assert_eq!(
            Dialect::resolve_local(None, &mars_src),
            Dialect::MarsNative
        );

        let staged = stage_local_item(
            &skill,
            ItemKind::Skill,
            Dialect::MarsNative,
            &IndexMap::new(),
            &root.path().join("staging"),
            "skill:demo",
            Some("demo"),
        )
        .unwrap();

        let content = fs::read_to_string(staged.join("SKILL.md")).unwrap();
        assert!(content.contains("disable-model-invocation"));

        let mut diags = Vec::new();
        crate::compiler::skills::parse_skill_content(&content, &mut diags).unwrap();
        assert!(diags.iter().any(|d| d.is_error()));
    }

    #[test]
    fn local_claude_container_infers_claude_and_lifts_foreign_keys() {
        let root = TempDir::new().unwrap();
        let mars_src = root.path().join(".mars-src");
        let skill = mars_src.join(".claude/skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        assert_eq!(Dialect::resolve_local(None, &mars_src), Dialect::Claude);

        let staged = stage_local_item(
            &skill,
            ItemKind::Skill,
            Dialect::Claude,
            &IndexMap::new(),
            &root.path().join("staging"),
            "skill:demo",
            Some("demo"),
        )
        .unwrap();

        let content = fs::read_to_string(staged.join("SKILL.md")).unwrap();
        assert!(!content.contains("disable-model-invocation"));
        assert!(content.contains("model-invocable: false"));
    }
}
