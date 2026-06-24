//! Apply `[skills.<name>]` overlays after dialect lift (Phase C-skills).

use std::path::Path;

use serde_yaml::Value;

use crate::compiler::tool_policy;
use crate::config::SkillOverlay;
use crate::frontmatter::Frontmatter;
use crate::skill_source_name::flat_root_skill_source_name;
use crate::types::RenameMap;

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

    if !overlay.tools.allowed.is_empty() {
        changed |= set_string_list_field(
            &mut merged,
            "tools",
            &overlay.tools.allowed,
            &tool_policy::non_canonical_aliases_for("tools"),
        );
    }
    if !overlay.tools.disallowed.is_empty() {
        changed |= set_string_list_field(
            &mut merged,
            "disallowed-tools",
            &overlay.tools.disallowed,
            &tool_policy::non_canonical_aliases_for("disallowed-tools"),
        );
    }
    (merged, changed)
}

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

fn set_string_list_field(
    fm: &mut Frontmatter,
    key: &str,
    values: &[String],
    aliases: &[&str],
) -> bool {
    if string_list_field(fm, key, aliases) == values {
        return false;
    }
    fm.insert(
        key,
        Value::Sequence(
            values
                .iter()
                .map(|value| Value::String(value.clone()))
                .collect(),
        ),
    );
    for alias in aliases {
        fm.remove(alias);
    }
    true
}

fn string_list_field(fm: &Frontmatter, key: &str, aliases: &[&str]) -> Vec<String> {
    fm.get(key)
        .or_else(|| aliases.iter().find_map(|alias| fm.get(alias)))
        .map(yaml_string_list)
        .unwrap_or_default()
}

fn yaml_string_list(value: &Value) -> Vec<String> {
    match value {
        Value::Sequence(seq) => seq
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentOverlayTools;
    use crate::types::ItemName;

    fn fm(yaml: &str) -> Frontmatter {
        Frontmatter::parse(&format!("---\n{yaml}---\n# Body\n")).unwrap()
    }

    #[test]
    fn overlay_changes_description_and_tools() {
        let overlay = SkillOverlay {
            description: Some("Overridden".to_string()),
            user_invocable: Some(false),
            tools: AgentOverlayTools {
                disallowed: vec!["Agent".to_string()],
                ..AgentOverlayTools::default()
            },
            ..SkillOverlay::default()
        };
        let (merged, changed) = apply_skill_overlay(
            &fm("name: demo\ndescription: base\nuser-invocable: true\n"),
            &overlay,
        );
        assert!(changed);
        assert_eq!(
            merged.get("description"),
            Some(&Value::String("Overridden".into()))
        );
        assert_eq!(merged.get("user-invocable"), Some(&Value::Bool(false)));
        assert_eq!(
            merged.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("Agent".into())]))
        );
    }

    #[test]
    fn overlay_projects_allowed_tools_to_canonical_tools() {
        let overlay = SkillOverlay {
            tools: AgentOverlayTools {
                allowed: vec!["Bash(git *)".to_string()],
                ..AgentOverlayTools::default()
            },
            ..SkillOverlay::default()
        };
        let (merged, changed) =
            apply_skill_overlay(&fm("name: demo\ndescription: base\n"), &overlay);
        assert!(changed);
        assert_eq!(
            merged.get("tools"),
            Some(&Value::Sequence(vec![Value::String("Bash(git *)".into())]))
        );
        assert!(merged.get("allowed-tools").is_none());
    }

    #[test]
    fn overlay_idempotent_when_values_match() {
        let overlay = SkillOverlay {
            description: Some("Same".to_string()),
            user_invocable: Some(true),
            ..SkillOverlay::default()
        };
        let (_, changed) = apply_skill_overlay(
            &fm("name: demo\ndescription: Same\nuser-invocable: true\n"),
            &overlay,
        );
        assert!(!changed);
    }

    #[test]
    fn overlay_lookup_uses_installed_name_after_rename() {
        let package = std::path::Path::new("/pkg");
        let skill_md = package.join("skills/planning/SKILL.md");
        let mut renames = RenameMap::new();
        renames.insert(
            ItemName::from("planning"),
            ItemName::from("research-planning"),
        );

        assert_eq!(
            skill_overlay_lookup_name(&skill_md, package, &renames, None).as_deref(),
            Some("research-planning")
        );
    }

    #[test]
    fn overlay_lookup_flat_skill_uses_package_basename_without_fallback() {
        let package = std::path::Path::new("/pkg/my-skill");
        let skill_md = package.join("SKILL.md");

        assert_eq!(
            skill_overlay_lookup_name(&skill_md, package, &RenameMap::new(), None).as_deref(),
            Some("my-skill")
        );
    }

    #[test]
    fn overlay_lookup_flat_skill_uses_fallback_name() {
        let package = std::path::Path::new("/pkg");
        let skill_md = package.join("SKILL.md");

        assert_eq!(
            skill_overlay_lookup_name(&skill_md, package, &RenameMap::new(), Some("my-skill"))
                .as_deref(),
            Some("my-skill")
        );
    }

    #[test]
    fn overlay_lookup_rename_applies_to_flat_skill() {
        let package = std::path::Path::new("/pkg");
        let skill_md = package.join("SKILL.md");
        let mut renames = RenameMap::new();
        renames.insert(ItemName::from("base"), ItemName::from("renamed-skill"));

        assert_eq!(
            skill_overlay_lookup_name(&skill_md, package, &renames, Some("base")).as_deref(),
            Some("renamed-skill")
        );
    }
}
