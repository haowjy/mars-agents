//! The serialized v1 reading shared by native and WASM callers.

use serde::Serialize;
use serde_yaml::Value;

use crate::diagnostic::SchemaDiagnostic;

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
    pub frontmatter: Vec<(String, serde_json::Value)>,
    pub fields: Option<T>,
    pub body: Option<String>,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

pub(crate) fn frontmatter_json(
    mapping: &serde_yaml::Mapping,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Vec<(String, serde_json::Value)> {
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
            match json_value(value) {
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

pub(crate) fn json_value(value: &Value) -> Result<serde_json::Value, String> {
    match value {
        Value::Null => Ok(serde_json::Value::Null),
        Value::Bool(v) => Ok((*v).into()),
        Value::Number(v) => serde_json::to_value(v).map_err(|e| e.to_string()),
        Value::String(v) => Ok(v.clone().into()),
        Value::Sequence(values) => values
            .iter()
            .map(json_value)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        Value::Mapping(values) => {
            let mut out = serde_json::Map::new();
            for (key, value) in values {
                let Some(key) = key.as_str() else {
                    return Err("YAML mapping contains a non-string key".into());
                };
                out.insert(key.to_owned(), json_value(value)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        Value::Tagged(_) => Err("YAML tags have no JSON representation".into()),
    }
}
