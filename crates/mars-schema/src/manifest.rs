//! Package-manifest reading owned by the schema boundary.
//!
//! The reading's `frontmatter` holds the `[package]` table's pairs, unknown keys included.

use crate::package::PackageMetadata;

/// Read package identity while leaving consumer-only top-level tables outside the schema.
pub fn read(content: &str) -> crate::read::DefinitionRead<PackageMetadata> {
    use crate::diagnostic::{SchemaDiagnostic, Severity, child_path, key_path};
    let value: toml::Value = match toml::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return crate::read::DefinitionRead {
                frontmatter: Vec::new(),
                fields: None,
                body: None,
                diagnostics: vec![SchemaDiagnostic::new(
                    "toml.invalid",
                    Severity::Error,
                    Vec::new(),
                    error.to_string(),
                )],
            };
        }
    };
    let Some(package) = value.get("package").and_then(toml::Value::as_table) else {
        return crate::read::DefinitionRead {
            frontmatter: Vec::new(),
            fields: None,
            body: None,
            diagnostics: vec![SchemaDiagnostic::new(
                "manifest.package-required",
                Severity::Error,
                key_path("package"),
                "manifest requires a [package] table",
            )],
        };
    };
    let known = [
        "name",
        "version",
        "description",
        "schema",
        "requires-mars",
        "requires-meridian",
        "authors",
        "homepage",
        "license",
        "uses-providers",
    ];
    let mut diagnostics = Vec::new();
    for key in package.keys() {
        if key == "visibility" {
            diagnostics.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Error,
                child_path("package", "visibility"),
                "`package.visibility` was removed; remove it",
            ));
        } else if !known.contains(&key.as_str()) {
            diagnostics.push(SchemaDiagnostic::new(
                "field.unknown",
                Severity::Warning,
                child_path("package", key),
                format!("unknown package field `{key}` is preserved"),
            ));
        }
    }
    if !matches!(package.get("schema"), Some(toml::Value::Integer(1))) {
        diagnostics.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            child_path("package", "schema"),
            "`package.schema` must be the integer 1",
        ));
    }
    if !matches!(package.get("name"), Some(toml::Value::String(value)) if !value.trim().is_empty())
    {
        diagnostics.push(SchemaDiagnostic::new(
            "manifest.name-required",
            Severity::Error,
            child_path("package", "name"),
            "package name must be a non-empty string",
        ));
    }
    match package.get("version") {
        None => diagnostics.push(SchemaDiagnostic::new(
            "manifest.version-missing",
            Severity::Warning,
            child_path("package", "version"),
            "package version is missing",
        )),
        Some(toml::Value::String(_)) => {}
        Some(_) => diagnostics.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            child_path("package", "version"),
            "`package.version` must be a string",
        )),
    }
    let fields = match toml::Value::Table(package.clone()).try_into::<PackageMetadata>() {
        Ok(fields) => Some(fields),
        Err(error) => {
            if !diagnostics.iter().any(SchemaDiagnostic::is_error) {
                diagnostics.push(SchemaDiagnostic::new(
                    "manifest.invalid-field",
                    Severity::Error,
                    key_path("package"),
                    format!("invalid package field: {error}"),
                ));
            }
            None
        }
    };
    crate::read::DefinitionRead {
        frontmatter: package
            .iter()
            .map(|(key, value)| (key.clone(), toml_json(value)))
            .collect(),
        fields,
        body: None,
        diagnostics,
    }
}

fn toml_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(v) => v.clone().into(),
        toml::Value::Integer(v) => (*v).into(),
        toml::Value::Float(v) => serde_json::Number::from_f64(*v)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::Boolean(v) => (*v).into(),
        toml::Value::Datetime(v) => v.to_string().into(),
        toml::Value::Array(values) => values.iter().map(toml_json).collect(),
        toml::Value::Table(table) => table
            .iter()
            .map(|(key, value)| (key.clone(), toml_json(value)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
    }
}
