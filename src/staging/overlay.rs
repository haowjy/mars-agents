//! Apply `[skills.<name>]` overlays after dialect lift (Phase C-skills).

use std::path::Path;

use serde_yaml::Value;

use crate::config::SkillOverlay;
use crate::skill_source_name::flat_root_skill_source_name;
use mars_schema::frontmatter::Frontmatter;
use mars_schema::names::RenameMap;

/// Installed skill name for `[skills.<name>]` overlay lookup.
///
/// Keys match the post-materialization name users configure in mars.toml
/// (after explicit rename), not the source directory basename alone.
pub(crate) fn skill_overlay_lookup_name(
    skill_md_path: &Path,
    package_root: &Path,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
) -> Option<String> {
    let source_name = skill_source_name(skill_md_path, package_root, fallback_skill_name)?;
    Some(installed_skill_name(&source_name, renames))
}

fn skill_source_name(
    skill_md_path: &Path,
    package_root: &Path,
    fallback_skill_name: Option<&str>,
) -> Option<String> {
    if skill_md_path == package_root.join("SKILL.md") {
        return Some(flat_root_skill_source_name(
            package_root,
            fallback_skill_name,
        ));
    }

    skill_md_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

fn installed_skill_name(source_name: &str, renames: &RenameMap) -> String {
    let default_key = format!("skills/{source_name}");
    match renames
        .get(default_key.as_str())
        .or_else(|| renames.get(source_name))
    {
        Some(dest) => skill_name_from_rename_dest(dest.as_str()),
        None => source_name.to_string(),
    }
}

fn skill_name_from_rename_dest(rename_value: &str) -> String {
    let normalized = rename_value.replace('\\', "/");
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_string()
}

/// Merge overlay fields into lifted canonical frontmatter.
///
/// Returns whether any field value changed. When `false`, callers should preserve
/// source bytes (idempotent staging).
pub(crate) fn apply_skill_overlay(
    frontmatter: &Frontmatter,
    overlay: &SkillOverlay,
) -> (Frontmatter, bool) {
    if overlay.is_empty() {
        return (frontmatter.clone(), false);
    }

    let mut merged = frontmatter.clone();
    let mut changed = false;

    if let Some(description) = &overlay.description {
        changed |= set_string_field(&mut merged, "description", description);
    }

    if let Some(model_invocable) = overlay.model_invocable {
        changed |= set_bool_field(
            &mut merged,
            "model-invocable",
            model_invocable,
            &["model_invocable"],
        );
    }

    if let Some(user_invocable) = overlay.user_invocable {
        changed |= set_bool_field(
            &mut merged,
            "user-invocable",
            user_invocable,
            &["user_invocable"],
        );
    }

    (merged, changed)
}

/// Set the scalar skill fields that consumer overlays may author.
fn set_string_field(fm: &mut Frontmatter, key: &str, value: &str) -> bool {
    if fm.get(key).and_then(Value::as_str) == Some(value) {
        return false;
    }
    fm.insert(key, Value::String(value.to_string()));
    true
}

fn set_bool_field(fm: &mut Frontmatter, key: &str, value: bool, aliases: &[&str]) -> bool {
    if fm.get(key).and_then(Value::as_bool) == Some(value) {
        return false;
    }
    fm.insert(key, Value::Bool(value));
    for alias in aliases {
        fm.remove(alias);
    }
    true
}
