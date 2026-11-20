//! Package-manifest reading owned by the schema boundary.
//!
//! The reading's `frontmatter` holds the `[package]` table's pairs, unknown keys included.

use crate::config::Settings;
use crate::dependency::Dependencies;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, child_path, key_path};
use crate::ordered::{MAX_SAFE_INTEGER, OrderedValue};
use crate::package::PackageMetadata;
use crate::read::DefinitionRead;
use serde::Serialize;

/// The reading of a `mars.toml`: the `[package]` table as every definition reading has it, the
/// dependency entries the package declares, and, from [`read_all`], the consumer tables. Table
/// keys are the file's names and are left out when empty; the fields inside `settings`, `agents`
/// and `models` are spelled as Mars writes them back (`default_effort`). The
/// consumer tables are plain JSON values, the same as the `[package]` pairs: a key or value JSON
/// cannot hold is left out with a `toml.non-json-value` diagnostic.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestRead {
    #[serde(flatten)]
    pub package: DefinitionRead<PackageMetadata>,
    #[serde(flatten)]
    pub dependencies: Dependencies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<OrderedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<OrderedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<OrderedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<OrderedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<OrderedValue>,
}

/// Read package identity and dependency entries while leaving consumer-only top-level tables
/// outside the schema. A package with an error has no dependencies: a dependent installs from
/// what is declared, so it is refused whole.
pub fn read(content: &str) -> ManifestRead {
    let (value, document) = match parse(content) {
        Ok(parsed) => parsed,
        Err(refusal) => return ManifestRead::unread(vec![refusal]),
    };
    let mut reading = read_package(&document, &value);
    let dependencies = if reading.fields.is_some() {
        Dependencies::read(&document, &mut reading.diagnostics)
    } else {
        Dependencies::default()
    };
    ManifestRead {
        package: reading,
        dependencies,
        ..ManifestRead::unread(Vec::new())
    }
}

/// [`read`], and the consumer tables beside `[package]` and the dependencies: what a project's
/// `mars.toml` and the CLI's loader read. The file is read as the consumer configuration it is:
/// `[package]` is optional, and the dependencies are returned with or without it. Each table
/// that is valid on its own is returned; one that breaks a rule is left out with a
/// `config.invalid` error whose path names it (`models.a`, `settings`), and the others stay. A
/// `[settings]` refusal (`[settings.tools]` with a `*`, a `min_mars_version` that is not a
/// requirement) is an error too, the table kept. `read` itself never fails on a consumer table,
/// because a dependent reads only a package's `[models]` and refuses nothing else.
pub fn read_all(content: &str) -> ManifestRead {
    let (value, document) = match parse(content) {
        Ok(parsed) => parsed,
        Err(refusal) => return ManifestRead::unread(vec![refusal]),
    };
    let mut package = if value.get("package").is_some() {
        read_package(&document, &value)
    } else {
        DefinitionRead::unread(Vec::new())
    };
    let dependencies = Dependencies::read(&document, &mut package.diagnostics);
    let mut reading = ManifestRead {
        package,
        dependencies,
        ..ManifestRead::unread(Vec::new())
    };
    let consumer = crate::config::read_consumer(content);
    let diagnostics = &mut reading.package.diagnostics;
    let reported = diagnostics.iter().any(SchemaDiagnostic::is_error);
    // The whole-file pass can fail on a value the package or dependency reading already
    // refused; when nothing else says why it failed, its own text does.
    if let Some(error) = &consumer.parse_error
        && consumer.issues.is_empty()
        && !reported
    {
        diagnostics.push(invalid(Vec::new(), error.to_string()));
    }
    diagnostics.extend(
        consumer
            .issues
            .into_iter()
            .map(|issue| invalid(issue.path.clone(), issue.message)),
    );
    let config = consumer.config;
    diagnostics.extend(config.unknown_keys.iter().cloned());
    reading.plugin = plugin_json(&document, diagnostics);
    reading.settings = (config.settings != Settings::default())
        .then(|| safe_table("settings", &config.settings, diagnostics))
        .flatten();
    reading.models = safe_table("models", &config.models, diagnostics);
    reading.agents = safe_table("agents", &config.agents, diagnostics);
    reading.skills = safe_table("skills", &config.skills, diagnostics);
    reading
}

fn invalid(path: Vec<PathSegment>, message: String) -> SchemaDiagnostic {
    SchemaDiagnostic::new("config.invalid", Severity::Error, path, message)
}

/// `content` as TOML, or the diagnostic that says it is not.
fn parse(content: &str) -> Result<(toml::Value, toml_edit::DocumentMut), SchemaDiagnostic> {
    let value: toml::Value = toml::from_str(content).map_err(|error| {
        SchemaDiagnostic::new(
            "toml.invalid",
            Severity::Error,
            Vec::new(),
            error.to_string(),
        )
    })?;
    // `content` parsed as TOML above, so it parses here.
    let document = content
        .parse::<toml_edit::DocumentMut>()
        .expect("content that parses as TOML parses as a document");
    Ok((value, document))
}

/// A consumer table as a plain JSON value in its written order, `None` when it holds nothing.
/// What the typed tables can carry beyond strings and small integers is a key, so a `__proto__`
/// at any depth is left out with a diagnostic.
fn safe_table(
    table: &str,
    value: &impl Serialize,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Option<OrderedValue> {
    // Serialization order is the written order; `serde_json::Value` would sort it.
    let text = serde_json::to_string(value).expect("a config table serializes");
    let mut value: OrderedValue =
        serde_json::from_str(&text).expect("JSON text this crate wrote reads back");
    strip_unholdable_keys(&mut value, &mut key_path(table), diagnostics);
    match &value {
        OrderedValue::Object(map) if map.is_empty() => None,
        _ => Some(value),
    }
}

fn strip_unholdable_keys(
    value: &mut OrderedValue,
    at: &mut Vec<PathSegment>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) {
    match value {
        OrderedValue::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                at.push(PathSegment::Key(key.clone()));
                if let Err(message) = reject_key(&key) {
                    map.shift_remove(&key);
                    diagnostics.push(SchemaDiagnostic::new(
                        "toml.non-json-value",
                        Severity::Error,
                        at.clone(),
                        message,
                    ));
                } else if let Some(inner) = map.get_mut(&key) {
                    strip_unholdable_keys(inner, at, diagnostics);
                }
                at.pop();
            }
        }
        OrderedValue::Array(values) => {
            for (index, inner) in values.iter_mut().enumerate() {
                at.push(PathSegment::Index(index));
                strip_unholdable_keys(inner, at, diagnostics);
                at.pop();
            }
        }
        _ => {}
    }
}

/// `[plugin]` as written: its pairs the way the `[package]` pairs are read, or the whole value
/// when it is not a table.
fn plugin_json(
    document: &toml_edit::DocumentMut,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Option<OrderedValue> {
    let item = document.get("plugin")?;
    if item.as_table_like().is_some() {
        return Some(OrderedValue::Object(
            table_pairs(document, "plugin", diagnostics)
                .into_iter()
                .collect(),
        ));
    }
    match item_json(item) {
        Ok(value) => Some(value),
        Err(message) => {
            diagnostics.push(SchemaDiagnostic::new(
                "toml.non-json-value",
                Severity::Error,
                crate::diagnostic::key_path("plugin"),
                message,
            ));
            None
        }
    }
}

impl ManifestRead {
    /// The first error of the reading: a manifest with any error is refused whole.
    pub fn first_error(&self) -> Option<&SchemaDiagnostic> {
        self.package
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.is_error())
    }

    /// A manifest that could not be read: no fields, only why.
    pub fn unread(diagnostics: Vec<SchemaDiagnostic>) -> Self {
        Self {
            package: DefinitionRead::unread(diagnostics),
            dependencies: Dependencies::default(),
            plugin: None,
            settings: None,
            models: None,
            agents: None,
            skills: None,
        }
    }
}

fn read_package(
    document: &toml_edit::DocumentMut,
    value: &toml::Value,
) -> DefinitionRead<PackageMetadata> {
    let Some(package) = value.get("package").and_then(toml::Value::as_table) else {
        return DefinitionRead::unread(vec![SchemaDiagnostic::new(
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
        "tools",
    ];
    // A newer schema's other fields may mean something this Mars cannot read, so the one thing
    // to say is to upgrade.
    if let Some(toml::Value::Integer(schema @ 2..)) = package.get("schema") {
        return DefinitionRead::unread(vec![SchemaDiagnostic::new(
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
        if known.contains(&key.as_str()) {
            continue;
        }
        let path = child_path("package", key);
        diagnostics.push(
            crate::retired::diagnostic(crate::retired::Scope::Package, key, path.clone())
                .unwrap_or_else(|| crate::diagnostic::unknown_field(path, "package", key)),
        );
    }
    if let Some(toml::Value::Array(entries)) = package.get("uses-providers") {
        for (index, entry) in entries.iter().enumerate() {
            if let toml::Value::String(name) = entry
                && !crate::provider::is_provider_name(name)
            {
                let mut path = child_path("package", "uses-providers");
                path.push(PathSegment::Index(index));
                diagnostics.push(SchemaDiagnostic::new(
                    "field.value",
                    Severity::Error,
                    path,
                    format!(
                        "`package.uses-providers` entry `{name}` is not a provider name; a provider name matches [a-z0-9][a-z0-9-]*"
                    ),
                ));
            }
        }
    }
    if package.get("schema").is_none() {
        diagnostics.push(SchemaDiagnostic::new(
            "manifest.schema-missing",
            Severity::Error,
            child_path("package", "schema"),
            format!(
                "`[package]` has no `schema`: add `schema = 1`, or migrate the package with `mars migrate` from Mars 0.33.x ({}/retired-keys.md#package-without-schema)",
                crate::diagnostic::DOCS
            ),
        ));
    } else if !matches!(package.get("schema"), Some(toml::Value::Integer(1))) {
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
            Severity::Info,
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
        Ok(fields) => {
            if let Some(message) = fields.tools.wildcard_refusal(
                "[package.tools]",
                "an agent's own `'*'` rule is its default",
            ) {
                diagnostics.push(SchemaDiagnostic::new(
                    "field.value",
                    Severity::Error,
                    child_path("package", "tools"),
                    message,
                ));
            }
            Some(fields)
        }
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
    DefinitionRead {
        frontmatter: table_pairs(document, "package", &mut diagnostics),
        fields,
        body: None,
        diagnostics,
    }
}

/// The pairs of the table `name` in file order, at every depth. A value JSON cannot hold exactly
/// (an integer beyond ±(2^53 − 1), NaN, infinity, a `__proto__` key) leaves its pair out with a
/// `toml.non-json-value` diagnostic; a datetime is its TOML text, a string.
fn table_pairs(
    document: &toml_edit::DocumentMut,
    name: &str,
    diagnostics: &mut Vec<crate::diagnostic::SchemaDiagnostic>,
) -> Vec<(String, OrderedValue)> {
    use toml_edit::Item;
    let Some(package) = document.get(name).and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for (key, item) in package.iter() {
        match item_json(item).and_then(|value| reject_key(key).map(|()| value)) {
            Ok(value) => pairs.push((key.to_owned(), value)),
            Err(message) => diagnostics.push(SchemaDiagnostic::new(
                "toml.non-json-value",
                Severity::Error,
                child_path(name, key),
                message,
            )),
        }
    }
    pairs
}

fn reject_key(key: &str) -> Result<(), String> {
    crate::ordered::reject_unholdable_key(key)
}

pub(crate) fn item_json(item: &toml_edit::Item) -> Result<OrderedValue, String> {
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
