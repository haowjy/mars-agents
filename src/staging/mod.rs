//! Canonical source staging — lift foreign frontmatter before discovery/validation.
//!
//! `stage_canonical_source` copies a fetched package tree into a derived,
//! dependency-scoped directory and routes markdown frontmatter through
//! `lift_frontmatter`. Downstream pipeline stages read only the staged tree.

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_yaml::Value;

use crate::config::SkillOverlay;
use crate::dialect::{Dialect, EXPLICIT_DIALECT_MARKER};
use crate::error::MarsError;
use crate::frontmatter::Frontmatter;
use crate::lock::ItemKind;
use crate::platform::cache::safe_component;
use crate::resolve::RootedSourceRef;
use crate::types::SourceName;

/// Lift foreign frontmatter to canonical fields for the given dialect.
///
/// B-infra: identity for inferred/default paths; when dialect is explicitly
/// configured (non-`MarsNative`), records `_mars-inbound-dialect` so dialect
/// changes invalidate staged hashes before real lift tables land in B3.
pub fn lift_frontmatter(
    dialect: Dialect,
    _item_kind: ItemKind,
    frontmatter: &Frontmatter,
    explicit_dialect: bool,
) -> Frontmatter {
    if !explicit_dialect || dialect == Dialect::MarsNative {
        return frontmatter.clone();
    }

    let mut lifted = frontmatter.clone();
    lifted.insert(
        EXPLICIT_DIALECT_MARKER,
        Value::String(dialect.as_str().to_string()),
    );
    lifted
}

/// Stage a package tree and repoint `package_root` at the staged output.
pub fn stage_rooted_source(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    dialect: Dialect,
    explicit_dialect: bool,
    _skill_overrides: &IndexMap<String, SkillOverlay>,
    staging_root: &Path,
) -> Result<RootedSourceRef, MarsError> {
    let staged_package_root = staging_dir_for(staging_root, source_name, dialect);
    stage_canonical_source(
        &rooted.package_root,
        &staged_package_root,
        dialect,
        explicit_dialect,
        _skill_overrides,
    )?;
    Ok(RootedSourceRef {
        checkout_root: rooted.checkout_root,
        package_root: staged_package_root,
    })
}

/// Copy `source_root` into `dest_root`, rewriting frontmatter through lift.
pub fn stage_canonical_source(
    source_root: &Path,
    dest_root: &Path,
    dialect: Dialect,
    explicit_dialect: bool,
    _skill_overrides: &IndexMap<String, SkillOverlay>,
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
        explicit_dialect,
    )
}

/// Stage a single local item path (agent file or skill directory).
pub fn stage_local_item(
    source_path: &Path,
    kind: ItemKind,
    dialect: Dialect,
    explicit_dialect: bool,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    staging_root: &Path,
    item_key: &str,
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
                dialect,
                explicit_dialect,
            )?;
            Ok(dest_file)
        }
        ItemKind::Skill | ItemKind::BootstrapDoc => {
            stage_canonical_source(
                source_path,
                &dest,
                dialect,
                explicit_dialect,
                skill_overrides,
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

fn copy_and_lift_tree(
    source_root: &Path,
    dest_root: &Path,
    current: &Path,
    dialect: Dialect,
    explicit_dialect: bool,
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
            copy_and_lift_tree(source_root, dest_root, &src_path, dialect, explicit_dialect)?;
        } else if should_lift_markdown(&src_path) {
            let kind = item_kind_for_markdown(&src_path);
            process_markdown_file(&src_path, &dest_path, kind, dialect, explicit_dialect)?;
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
    ) || path.extension().is_some_and(|ext| ext == "md")
}

fn item_kind_for_markdown(path: &Path) -> ItemKind {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("SKILL.md") => ItemKind::Skill,
        Some("BOOTSTRAP.md") => ItemKind::BootstrapDoc,
        _ => ItemKind::Agent,
    }
}

fn process_markdown_file(
    src: &Path,
    dest: &Path,
    kind: ItemKind,
    dialect: Dialect,
    explicit_dialect: bool,
) -> Result<(), MarsError> {
    let original = fs::read_to_string(src)?;
    let needs_rewrite = explicit_dialect && dialect != Dialect::MarsNative;
    if !needs_rewrite {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dest)?;
        return Ok(());
    }

    let parsed = Frontmatter::parse(&original).map_err(|e| MarsError::Source {
        source_name: "staging".to_string(),
        message: format!("failed to parse frontmatter in {}: {e}", src.display()),
    })?;
    let lifted = lift_frontmatter(dialect, kind, &parsed, explicit_dialect);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest, lifted.render())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;
    use tempfile::TempDir;

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
            false,
            &IndexMap::new(),
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
    fn explicit_dialect_marker_invalidates_staged_hash() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: demo\n---\n# Body\n").unwrap();

        let native = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            native.path(),
            Dialect::MarsNative,
            true,
            &IndexMap::new(),
        )
        .unwrap();

        let claude = TempDir::new().unwrap();
        stage_canonical_source(
            source.path(),
            claude.path(),
            Dialect::Claude,
            true,
            &IndexMap::new(),
        )
        .unwrap();

        let native_hash =
            hash::compute_hash(&native.path().join("skills/demo"), ItemKind::Skill).unwrap();
        let claude_hash =
            hash::compute_hash(&claude.path().join("skills/demo"), ItemKind::Skill).unwrap();
        assert_ne!(native_hash, claude_hash);
    }
}
