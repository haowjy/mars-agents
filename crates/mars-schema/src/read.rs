//! The serialized v1 reading shared by native and WASM callers.

use serde::Serialize;

use crate::diagnostic::SchemaDiagnostic;
use crate::ordered::OrderedValue;

/// The serialized v1 reading of one schema-owned file.
///
/// Serializes as the bare [`DefinitionRead`]; the caller knows the kind from the path.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Reading {
    Manifest(DefinitionRead<crate::package::PackageMetadata>),
    Agent(DefinitionRead<crate::agent::AgentProfile>),
    Skill(DefinitionRead<crate::skill::SkillProfile>),
}

/// `read` was given a path that is not a schema-owned file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a schema file: expected `mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md`"
)]
pub struct UnrecognizedPath(pub String);

/// Read one schema-owned file, choosing the reader from its package-relative path.
pub fn read(path: &str, text: &str) -> Result<Reading, UnrecognizedPath> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["mars.toml"] => Ok(Reading::Manifest(crate::manifest::read(text))),
        ["agents", file]
            if file
                .strip_suffix(".md")
                .is_some_and(|slug| !slug.is_empty()) =>
        {
            Ok(Reading::Agent(crate::agent::read(text)))
        }
        ["skills", slug, "SKILL.md"] if !slug.is_empty() => {
            Ok(Reading::Skill(crate::skill::read(text)))
        }
        _ => Err(UnrecognizedPath(path.to_owned())),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DefinitionRead<T> {
    /// Ordered top-level mapping, retaining unknown authored keys.
    pub frontmatter: Vec<(String, OrderedValue)>,
    pub fields: Option<T>,
    pub body: Option<String>,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

pub(crate) fn frontmatter_json(
    mapping: &serde_yaml::Mapping,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Vec<(String, OrderedValue)> {
    mapping
        .iter()
        .filter_map(|(key, value)| {
            let Some(key) = key.as_str() else {
                diagnostics.push(SchemaDiagnostic::new(
                    "yaml.non-string-key",
                    crate::diagnostic::Severity::Error,
                    Vec::new(),
                    "frontmatter keys must be strings",
                ));
                return None;
            };
            match OrderedValue::from_yaml(value) {
                Ok(value) => Some((key.to_owned(), value)),
                Err(message) => {
                    diagnostics.push(SchemaDiagnostic::new(
                        "yaml.non-json-value",
                        crate::diagnostic::Severity::Error,
                        vec![crate::diagnostic::PathSegment::Key(key.to_owned())],
                        message,
                    ));
                    None
                }
            }
        })
        .collect()
}
