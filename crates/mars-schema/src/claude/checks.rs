//! Info about a translated skill that depends on where it landed: its name and description.

use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
use crate::frontmatter::Frontmatter;

pub(crate) fn skill_checks(frontmatter: &Frontmatter) -> Vec<SchemaDiagnostic> {
    let mut diagnostics = Vec::new();
    let info = |code: &str, path, message: String| {
        SchemaDiagnostic::new(code, Severity::Info, path, message)
    };
    if let Some(name) = frontmatter.name()
        && !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        diagnostics.push(info(
            "skill.name-invalid",
            key_path("name"),
            format!("name `{name}` has characters outside `a-z`, `0-9` and `-`"),
        ));
    }
    if frontmatter
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .is_some_and(|description| description.chars().count() > 1024)
    {
        diagnostics.push(info(
            "skill.description-too-long",
            key_path("description"),
            "description is over 1024 characters".into(),
        ));
    }
    diagnostics
}
