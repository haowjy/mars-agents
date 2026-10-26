//! The one save path for `mars.toml` and `mars.local.toml`: apply the base→next change to the
//! authored document.
//!
//! Base is the file as it stands, read through the same parser as `load`; next is what the
//! command built. Only keys whose value differs between the two serialize are touched, so a key
//! the command did not change keeps its bytes, comments and order, and a key Mars does not model
//! (an unknown `[package]` or `[dependencies.x]` key) is never seen and never lost.

use std::path::Path;

use serde::Serialize;
use toml_edit::{DocumentMut, Item, TableLike, Value as EditValue};

use crate::error::{ConfigError, MarsError};

fn invalid(message: String) -> MarsError {
    ConfigError::Invalid { message }.into()
}

fn to_toml<T: Serialize>(config: &T) -> Result<toml::Table, MarsError> {
    toml::Table::try_from(config).map_err(|e| invalid(format!("failed to serialize config: {e}")))
}

pub(super) fn save_document<T: Serialize>(
    path: &Path,
    next: &T,
    parse: impl Fn(&str) -> Result<T, MarsError>,
) -> Result<(), MarsError> {
    let next_table = to_toml(next)?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let content = toml::to_string_pretty(&next_table)
                .map_err(|e| invalid(format!("failed to serialize config: {e}")))?;
            return crate::fs::atomic_write(path, content.as_bytes());
        }
        Err(e) => return Err(ConfigError::Io(e).into()),
    };

    let base_table = to_toml(&parse(&text)?)?;
    if base_table == next_table {
        return Ok(());
    }
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| invalid(format!("{}: {e}", path.display())))?;
    apply_table(doc.as_table_mut(), &base_table, &next_table);

    let content = doc.to_string();
    if to_toml(&parse(&content)?)? != next_table {
        return Err(invalid(format!(
            "refusing to save {}: the edit did not produce the requested config",
            path.display()
        )));
    }
    crate::fs::atomic_write(path, content.as_bytes())
}

fn apply_table(target: &mut dyn TableLike, base: &toml::Table, next: &toml::Table) {
    for key in base.keys().filter(|key| !next.contains_key(*key)) {
        target.remove(key);
    }
    for (key, next_value) in next {
        match (base.get(key), target.get_mut(key)) {
            (Some(base_value), _) if base_value == next_value => {}
            (Some(toml::Value::Table(base_table)), Some(item)) if next_value.is_table() => {
                match item.as_table_like_mut() {
                    Some(table) => apply_table(table, base_table, next_value.as_table().unwrap()),
                    None => *item = to_item(next_value),
                }
            }
            (Some(_), Some(item)) => replace(item, next_value),
            _ => {
                target.insert(key, to_item(next_value));
            }
        }
    }
}

/// Replace a value, keeping the whitespace and trailing comment around the old one.
fn replace(item: &mut Item, value: &toml::Value) {
    let mut new = to_item(value);
    if let (Some(old), Some(new_value)) = (item.as_value(), new.as_value_mut()) {
        *new_value.decor_mut() = old.decor().clone();
    }
    *item = new;
}

fn to_item(value: &toml::Value) -> Item {
    match value {
        toml::Value::Table(table) => {
            let mut out = toml_edit::Table::new();
            for (key, value) in table {
                out.insert(key, to_item(value));
            }
            // A table that only holds tables needs no header of its own (`[models]` above
            // `[models.fast]`).
            out.set_implicit(table.values().all(toml::Value::is_table));
            Item::Table(out)
        }
        other => Item::Value(to_value(other)),
    }
}

fn to_value(value: &toml::Value) -> EditValue {
    match value {
        toml::Value::String(s) => s.as_str().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(f) => (*f).into(),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Datetime(d) => d
            .to_string()
            .parse::<toml_edit::Datetime>()
            .expect("a TOML datetime reparses")
            .into(),
        toml::Value::Array(items) => items
            .iter()
            .map(to_value)
            .collect::<toml_edit::Array>()
            .into(),
        toml::Value::Table(table) => {
            let mut out = toml_edit::InlineTable::new();
            for (key, value) in table {
                out.insert(key, to_value(value));
            }
            out.into()
        }
    }
}
