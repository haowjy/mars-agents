//! Package-manifest reading owned by the schema boundary.

use serde::Deserialize;

use crate::package::PackageMetadata;

#[derive(Deserialize)]
struct PackageSection {
    package: Option<PackageMetadata>,
}

/// Read the optional `[package]` section while ignoring consumer-only tables.
pub fn read_package(content: &str) -> Result<Option<PackageMetadata>, toml::de::Error> {
    toml::from_str::<PackageSection>(content).map(|manifest| manifest.package)
}

/// Read package identity while leaving consumer-only top-level tables outside the schema.
pub fn read(content: &str) -> crate::read::DefinitionRead<PackageMetadata> {
    use crate::diagnostic::{SchemaDiagnostic, Severity, key_path};
    let value: toml::Value = match toml::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return crate::read::DefinitionRead {
                fields: None,
                body: None,
                unknown: Vec::new(),
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
            fields: None,
            body: None,
            unknown: Vec::new(),
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
    let mut unknown = Vec::new();
    for (key, value) in package {
        if key == "visibility" {
            diagnostics.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Error,
                key_path("package.visibility"),
                "`package.visibility` was removed; remove it",
            ));
        } else if !known.contains(&key.as_str()) {
            diagnostics.push(SchemaDiagnostic::new(
                "field.unknown",
                Severity::Warning,
                key_path(&format!("package.{key}")),
                format!("unknown package field `{key}` is preserved"),
            ));
            unknown.push((
                key.clone(),
                serde_yaml::to_value(value).expect("TOML values serialize"),
            ));
        }
    }
    if !matches!(package.get("schema"), Some(toml::Value::Integer(1))) {
        diagnostics.push(SchemaDiagnostic::new(
            "field.type",
            Severity::Error,
            key_path("package.schema"),
            "`package.schema` must be the integer 1",
        ));
    }
    if !matches!(package.get("name"), Some(toml::Value::String(value)) if !value.trim().is_empty())
    {
        diagnostics.push(SchemaDiagnostic::new(
            "manifest.name-required",
            Severity::Error,
            key_path("package.name"),
            "package name must be a non-empty string",
        ));
    }
    let fields = toml::from_str::<PackageSection>(content)
        .ok()
        .and_then(|manifest| manifest.package);
    crate::read::DefinitionRead {
        fields,
        body: None,
        unknown,
        diagnostics,
    }
}
