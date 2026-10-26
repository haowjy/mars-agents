//! Stable reader result shared by native and WASM callers.

use serde::Serialize;
use serde_yaml::Value;

use crate::diagnostic::SchemaDiagnostic;

/// One file returned by [`read_files`].
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", content = "reading", rename_all = "lowercase")]
pub enum FileRead {
    Manifest(DefinitionRead<crate::package::PackageMetadata>),
    Agent(DefinitionRead<crate::agent::AgentProfile>),
    Skill(DefinitionRead<crate::skill::SkillProfile>),
}

/// Read every schema-owned file in a package tree in one deterministic pass.
pub fn read_files(
    source: &impl crate::tree::TreeSource,
) -> std::collections::BTreeMap<String, FileRead> {
    let mut paths = Vec::new();
    fn walk(source: &impl crate::tree::TreeSource, dir: &str, paths: &mut Vec<String>) {
        for child in source.children(dir) {
            if source.is_file(&child) {
                if child == "mars.toml"
                    || child.starts_with("agents/")
                    || child.ends_with("/SKILL.md")
                    || child == "SKILL.md"
                {
                    paths.push(child);
                }
            } else if source.is_dir(&child) {
                walk(source, &child, paths);
            }
        }
    }
    walk(source, "", &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let bytes = source.read(&path)?;
            let text = String::from_utf8(bytes).ok()?;
            let result = if path == "mars.toml" {
                FileRead::Manifest(crate::manifest::read(&text))
            } else if path.starts_with("agents/") {
                FileRead::Agent(crate::agent::read(&text))
            } else {
                FileRead::Skill(crate::skill::read(&text))
            };
            Some((path, result))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct DefinitionRead<T> {
    /// Ordered top-level mapping, retaining unknown authored keys.
    pub frontmatter: Vec<(String, serde_json::Value)>,
    pub fields: Option<T>,
    pub body: Option<String>,
    pub unknown: Vec<(String, Value)>,
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

fn json_value(value: &Value) -> Result<serde_json::Value, String> {
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
