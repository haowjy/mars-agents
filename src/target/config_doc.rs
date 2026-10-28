//! A config file narrowed to one section, for MCP servers (JSON or TOML) and
//! hook events (JSON).
//!
//! MCP entries cross this boundary as JSON values: the one form mars renders,
//! records in the lock, and compares. A JSON file is held as an [`OrderedValue`]
//! so the user's key order survives a rewrite. A TOML file is edited in place
//! with `toml_edit`, so the user's other keys, comments and order survive. A
//! save writes through a symlink to the file it points at and keeps the file's
//! permissions.

use std::path::Path;

use indexmap::IndexMap;
use mars_schema::ordered::OrderedValue;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table};

use crate::error::{ConfigError, MarsError};

/// How the file holds its section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionFormat {
    /// A JSON object at this top-level key maps entry name to entry.
    JsonSection(&'static str),
    /// A TOML table at this top-level key holds one `[key.<name>]` table per entry.
    TomlTable(&'static str),
}

/// A JSON file, narrowed to the object at `key`. Hook config files use this
/// directly: their section maps event to an array, which the section accessors
/// edit in place.
pub(super) struct JsonDoc {
    root: IndexMap<String, OrderedValue>,
    key: &'static str,
}

/// The file, narrowed to its section at `key`. Each variant holds its
/// format's document, already checked to have the expected shape.
pub(super) enum Doc {
    Json(JsonDoc),
    Toml { doc: DocumentMut, key: &'static str },
}

fn invalid(message: String) -> MarsError {
    MarsError::Config(ConfigError::Invalid { message })
}

impl JsonDoc {
    /// Read `path`; `None` when it does not exist. A file that is not a JSON
    /// object, or whose `key` is not an object, is an error, so nothing is
    /// written over it.
    pub(super) fn load(path: &Path, key: &'static str) -> Result<Option<Self>, MarsError> {
        if !path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)?;
        let root = serde_json::from_str::<OrderedValue>(&raw)
            .map_err(|error| invalid(format!("{} is not valid JSON: {error}", path.display())))?;
        let OrderedValue::Object(root) = root else {
            return Err(invalid(format!("{} is not a JSON object", path.display())));
        };
        if root
            .get(key)
            .is_some_and(|value| !matches!(value, OrderedValue::Object(_)))
        {
            return Err(invalid(format!(
                "{}: {key} is not an object",
                path.display()
            )));
        }
        Ok(Some(Self { root, key }))
    }

    pub(super) fn new(key: &'static str) -> Self {
        Self {
            root: IndexMap::new(),
            key,
        }
    }

    pub(super) fn section(&self) -> Option<&IndexMap<String, OrderedValue>> {
        match self.root.get(self.key)? {
            OrderedValue::Object(section) => Some(section),
            _ => None,
        }
    }

    /// The section, created if absent.
    pub(super) fn section_mut(&mut self) -> &mut IndexMap<String, OrderedValue> {
        let OrderedValue::Object(section) = self
            .root
            .entry(self.key.to_owned())
            .or_insert_with(|| OrderedValue::Object(IndexMap::new()))
        else {
            unreachable!("validated on load")
        };
        section
    }

    /// Drop the section if it holds nothing.
    pub(super) fn drop_section_if_empty(&mut self) {
        if self.section().is_some_and(IndexMap::is_empty) {
            self.root.shift_remove(self.key);
        }
    }

    /// Set a top-level key outside the section.
    pub(super) fn set_top_level(&mut self, key: &str, value: Value) {
        self.root.insert(key.to_owned(), to_ordered(value));
    }

    fn get(&self, name: &str) -> Option<Value> {
        Some(self.section()?.get(name)?.to_json())
    }

    fn is_empty(&self) -> bool {
        self.root
            .iter()
            .all(|(k, v)| k == self.key && matches!(v, OrderedValue::Object(map) if map.is_empty()))
    }

    fn insert(&mut self, name: &str, value: Value) {
        self.section_mut()
            .insert(name.to_owned(), to_ordered(value));
    }

    fn remove(&mut self, name: &str) -> bool {
        let removed = self
            .root
            .get_mut(self.key)
            .and_then(|section| match section {
                OrderedValue::Object(section) => section.shift_remove(name),
                _ => None,
            })
            .is_some();
        if removed {
            self.drop_section_if_empty();
        }
        removed
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), MarsError> {
        let mut json = serde_json::to_string_pretty(&self.root)
            .map_err(|e| invalid(format!("failed to serialize {}: {e}", path.display())))?;
        json.push('\n');
        write_through(path, json.as_bytes())
    }
}

impl Doc {
    /// Read `path`; `None` when it does not exist. A file that is not the
    /// expected shape is an error, so nothing is written over it.
    pub(super) fn load(path: &Path, format: SectionFormat) -> Result<Option<Self>, MarsError> {
        match format {
            SectionFormat::JsonSection(key) => Ok(JsonDoc::load(path, key)?.map(Self::Json)),
            SectionFormat::TomlTable(key) => {
                if !path.is_file() {
                    return Ok(None);
                }
                let raw = std::fs::read_to_string(path)?;
                let doc = raw.parse::<DocumentMut>().map_err(|error| {
                    invalid(format!("{} is not valid TOML: {error}", path.display()))
                })?;
                if doc.get(key).is_some_and(|item| !item.is_table_like()) {
                    return Err(invalid(format!("{}: {key} is not a table", path.display())));
                }
                Ok(Some(Self::Toml { doc, key }))
            }
        }
    }

    pub(super) fn new(format: SectionFormat) -> Self {
        match format {
            SectionFormat::JsonSection(key) => Self::Json(JsonDoc::new(key)),
            SectionFormat::TomlTable(key) => Self::Toml {
                doc: DocumentMut::new(),
                key,
            },
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<Value> {
        match self {
            Self::Json(doc) => doc.get(name),
            Self::Toml { doc, key } => {
                Some(item_to_json(doc.get(key)?.as_table_like()?.get(name)?))
            }
        }
    }

    /// Nothing left but, at most, an empty section.
    pub(super) fn is_empty(&self) -> bool {
        match self {
            Self::Json(doc) => doc.is_empty(),
            // Comments live in decor, not items: require the rendered text to be
            // blank so a user's comment-only file is not deleted.
            Self::Toml { doc, .. } => doc.to_string().trim().is_empty(),
        }
    }

    pub(super) fn insert(&mut self, name: &str, value: Value) {
        match self {
            Self::Json(doc) => doc.insert(name, value),
            Self::Toml { doc, key } => {
                let section = doc.entry(key).or_insert_with(|| {
                    // Print only `[key.name]` headers, not an empty `[key]`.
                    let mut table = Table::new();
                    table.set_implicit(true);
                    Item::Table(table)
                });
                section
                    .as_table_like_mut()
                    .expect("validated on load")
                    .insert(name, json_to_item(&value));
            }
        }
    }

    pub(super) fn remove(&mut self, name: &str) -> bool {
        match self {
            Self::Json(doc) => doc.remove(name),
            Self::Toml { doc, key } => {
                let Some(section) = doc.get_mut(key).and_then(Item::as_table_like_mut) else {
                    return false;
                };
                let removed = section.remove(name).is_some();
                if removed && section.is_empty() {
                    doc.remove(key);
                }
                removed
            }
        }
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), MarsError> {
        match self {
            Self::Json(doc) => doc.save(path),
            Self::Toml { doc, .. } => write_through(path, doc.to_string().as_bytes()),
        }
    }
}

/// Write `content` to `path`. The file may be the user's, and a symlink to a
/// shared one: write the file it points at, so the link stays and the shared
/// file updates.
fn write_through(path: &Path, content: &[u8]) -> Result<(), MarsError> {
    let path = &resolve_symlinks(path)?;
    if path.is_file() {
        crate::fs::atomic_write_preserving_permissions(path, content)
    } else {
        crate::fs::atomic_write(path, content)
    }
}

/// Follow `path` through symlinks to the file at the end, which may not exist yet.
fn resolve_symlinks(path: &Path) -> Result<std::path::PathBuf, MarsError> {
    let mut current = path.to_path_buf();
    // The OS stops following at 40 links; so do we.
    for _ in 0..40 {
        if !current.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            return Ok(current);
        }
        let target = std::fs::read_link(&current)?;
        current = current
            .parent()
            .map_or(target.clone(), |dir| dir.join(&target));
    }
    Err(invalid(format!("{} is a symlink loop", path.display())))
}

pub(super) fn to_ordered(value: Value) -> OrderedValue {
    serde_json::from_value(value).expect("every JSON value is an OrderedValue")
}

fn json_to_item(value: &Value) -> Item {
    match value {
        // A server is a `[section.name]` table; anything nested stays inline.
        Value::Object(map) => {
            let mut table = Table::new();
            for (key, value) in map {
                table.insert(key, Item::Value(json_to_toml(value)));
            }
            Item::Table(table)
        }
        other => Item::Value(json_to_toml(other)),
    }
}

fn json_to_toml(value: &Value) -> toml_edit::Value {
    match value {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => n
            .as_i64()
            .map(Into::into)
            .or_else(|| n.as_f64().map(Into::into))
            .expect("JSON numbers are i64 or f64"),
        Value::Array(items) => items.iter().map(json_to_toml).collect(),
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in map {
                table.insert(key, json_to_toml(value));
            }
            table.into()
        }
        Value::Null => unreachable!("no mars render emits null, and TOML has none"),
    }
}

fn item_to_json(item: &Item) -> Value {
    match item {
        Item::None => Value::Null,
        Item::Value(value) => toml_to_json(value),
        Item::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.to_owned(), item_to_json(item)))
                .collect(),
        ),
        Item::ArrayOfTables(tables) => Value::Array(
            tables
                .iter()
                .map(|table| item_to_json(&Item::Table(table.clone())))
                .collect(),
        ),
    }
}

fn toml_to_json(value: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match value {
        T::String(s) => Value::String(s.value().clone()),
        T::Integer(i) => (*i.value()).into(),
        T::Float(f) => serde_json::Number::from_f64(*f.value()).map_or(Value::Null, Value::Number),
        T::Boolean(b) => Value::Bool(*b.value()),
        T::Datetime(d) => Value::String(d.value().to_string()),
        T::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        T::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.to_owned(), toml_to_json(value)))
                .collect(),
        ),
    }
}
