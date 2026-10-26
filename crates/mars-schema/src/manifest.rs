//! Package-manifest reading owned by the schema boundary.
//!
//! The reading's `frontmatter` holds the `[package]` table's pairs, unknown keys included.

use crate::diagnostic::{SchemaDiagnostic, Severity, child_path};
use crate::ordered::{MAX_SAFE_INTEGER, OrderedValue};
use crate::package::PackageMetadata;

/// Read package identity while leaving consumer-only top-level tables outside the schema.
pub fn read(content: &str) -> crate::read::DefinitionRead<PackageMetadata> {
    use crate::diagnostic::key_path;
    let value: toml::Value = match toml::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return crate::read::DefinitionRead::unread(vec![SchemaDiagnostic::new(
                "toml.invalid",
                Severity::Error,
                Vec::new(),
                error.to_string(),
            )]);
        }
    };
    let Some(package) = value.get("package").and_then(toml::Value::as_table) else {
        return crate::read::DefinitionRead::unread(vec![SchemaDiagnostic::new(
            "manifest.package-required",
            Severity::Error,
            key_path("package"),
            "manifest requires a [package] table",
        )]);
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
    // A newer schema's other fields may mean something this Mars cannot read, so the one thing
    // to say is to upgrade.
    if let Some(toml::Value::Integer(schema @ 2..)) = package.get("schema") {
        return crate::read::DefinitionRead::unread(vec![SchemaDiagnostic::new(
            "schema.newer",
            Severity::Error,
            child_path("package", "schema"),
            format!(
                "package declares schema {schema}; this Mars (v{}) reads schema 1 — upgrade Mars",
                crate::VERSION
            ),
        )]);
    }
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
        frontmatter: package_pairs(content, &mut diagnostics),
        fields,
        body: None,
        diagnostics,
    }
}

/// The `[package]` pairs in file order, at every depth. A value JSON cannot hold exactly (an
/// integer beyond ±(2^53 − 1), NaN, infinity, a `__proto__` key) leaves its pair out with a
/// `toml.non-json-value` diagnostic; a datetime is its TOML text, a string.
fn package_pairs(
    content: &str,
    diagnostics: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Vec<(String, OrderedValue)> {
    use toml_edit::{DocumentMut, Item};
    // `content` already parsed as TOML, and `[package]` exists.
    let Ok(document) = content.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let Some(package) = document.get("package").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for (key, item) in package.iter() {
        match item_json(item).and_then(|value| reject_key(key).map(|()| value)) {
            Ok(value) => pairs.push((key.to_owned(), value)),
            Err(message) => diagnostics.push(SchemaDiagnostic::new(
                "toml.non-json-value",
                Severity::Error,
                child_path("package", key),
                message,
            )),
        }
    }
    pairs
}

fn reject_key(key: &str) -> Result<(), String> {
    crate::ordered::reject_unholdable_key(key)
}

fn item_json(item: &toml_edit::Item) -> Result<OrderedValue, String> {
    use toml_edit::{Item, Value};
    Ok(match item {
        Item::None => OrderedValue::Null,
        Item::Value(Value::String(v)) => OrderedValue::String(v.value().clone()),
        Item::Value(Value::Integer(v)) => {
            let n = *v.value();
            if n.unsigned_abs() > MAX_SAFE_INTEGER {
                return Err(format!(
                    "integer {n} is beyond ±(2^53 − 1), which JavaScript cannot hold exactly"
                ));
            }
            OrderedValue::Number(n.into())
        }
        Item::Value(Value::Float(v)) => serde_json::Number::from_f64(*v.value())
            .map(OrderedValue::Number)
            .ok_or("NaN and infinity have no JSON representation")?,
        Item::Value(Value::Boolean(v)) => OrderedValue::Bool(*v.value()),
        Item::Value(Value::Datetime(v)) => OrderedValue::String(v.value().to_string()),
        Item::Value(Value::Array(values)) => OrderedValue::Array(
            values
                .iter()
                .map(|value| item_json(&Item::Value(value.clone())))
                .collect::<Result<_, _>>()?,
        ),
        Item::ArrayOfTables(tables) => OrderedValue::Array(
            tables
                .iter()
                .map(|table| item_json(&Item::Table(table.clone())))
                .collect::<Result<_, _>>()?,
        ),
        Item::Value(Value::InlineTable(_)) | Item::Table(_) => {
            let table = item.as_table_like().expect("a table item is table-like");
            let mut out = indexmap::IndexMap::new();
            for (key, item) in table.iter() {
                reject_key(key)?;
                out.insert(key.to_owned(), item_json(item)?);
            }
            OrderedValue::Object(out)
        }
    })
}
