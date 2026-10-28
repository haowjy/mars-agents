//! Warnings about a translated skill that depend on where it landed: its name and description.

use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
use crate::discover::SelectedDefinition;
use crate::frontmatter::Frontmatter;

pub(crate) fn skill_checks(
    definition: &SelectedDefinition,
    frontmatter: &Frontmatter,
) -> Vec<SchemaDiagnostic> {
    let mut diagnostics = Vec::new();
    let warn = |code: &str, path, message: String| {
        SchemaDiagnostic::new(code, Severity::Warning, path, message)
    };
    if let Some(name) = frontmatter.name() {
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            diagnostics.push(warn(
                "skill.name-invalid",
                key_path("name"),
                format!("name `{name}` has characters outside `a-z`, `0-9` and `-`"),
            ));
        } else if name != definition.slug {
            diagnostics.push(warn(
                "skill.name-mismatch",
                key_path("name"),
                format!(
                    "name `{name}` differs from the skill folder `{}`; the folder names it",
                    definition.slug
                ),
            ));
        }
    }
    if frontmatter
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .is_some_and(|description| description.chars().count() > 1024)
    {
        diagnostics.push(warn(
            "skill.description-too-long",
            key_path("description"),
            "description is over 1024 characters".into(),
        ));
    }
    diagnostics
}
