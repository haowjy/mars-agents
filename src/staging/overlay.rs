//! Apply `[skills.<name>]` overlays after dialect lift (Phase C-skills).

use serde_yaml::Value;

use crate::config::SkillOverlay;
use crate::frontmatter::Frontmatter;

/// Installed skill name for overlay lookup — parent directory of `SKILL.md`.
pub fn skill_installed_name(skill_md_path: &std::path::Path) -> Option<String> {
    skill_md_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

/// Merge overlay fields into lifted canonical frontmatter.
///
/// Returns whether any field value changed. When `false`, callers should preserve
/// source bytes (idempotent staging).
pub fn apply_skill_overlay(
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
        changed |= set_bool_field(&mut merged, "model-invocable", model_invocable, &[
            "model_invocable",
        ]);
    }

    if let Some(user_invocable) = overlay.user_invocable {
        changed |= set_bool_field(&mut merged, "user-invocable", user_invocable, &[
            "user_invocable",
        ]);
    }

    if !overlay.tools.allowed.is_empty() {
        changed |= set_string_list_field(
            &mut merged,
            "allowed-tools",
            &overlay.tools.allowed,
            &["allowed_tools"],
        );
    }
    if !overlay.tools.disallowed.is_empty() {
        changed |= set_string_list_field(
            &mut merged,
            "disallowed-tools",
            &overlay.tools.disallowed,
            &["disallowed_tools"],
        );
    }
    if !overlay.tools.mcp.is_empty() {
        changed |= set_string_list_field(
            &mut merged,
            "mcp-tools",
            &overlay.tools.mcp,
            &["mcp_tools"],
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
        assert_eq!(
            merged.get("user-invocable"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            merged.get("disallowed-tools"),
            Some(&Value::Sequence(vec![Value::String("Agent".into())]))
        );
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
    fn skill_installed_name_is_parent_directory() {
        let path = std::path::Path::new("/pkg/skills/planning/SKILL.md");
        assert_eq!(
            skill_installed_name(path).as_deref(),
            Some("planning")
        );
    }
}
