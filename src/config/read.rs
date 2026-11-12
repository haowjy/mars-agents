//! The one reader of the consumer config files (`mars.toml`, `mars.local.toml`, the user
//! `config.toml`).
//!
//! Serde reads the keys Mars knows. Every key it leaves unread becomes one note and stays in the
//! file. A key that was read once and is gone is no different from a typo, so nothing here names
//! a key.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde_ignored::Path as IgnoredPath;

use crate::error::{ConfigError, MarsError};
use mars_schema::keys;

/// A file read: the value, one warning for each key Mars did not read, and the Mars its tool
/// rules need.
pub(super) struct Read<T> {
    pub(super) value: T,
    pub(super) unknown_keys: Vec<String>,
    pub(super) floor: Option<keys::Floor>,
}

pub(super) enum Segment {
    Key(String),
    Index(usize),
}

pub(super) fn read<T: DeserializeOwned>(
    content: &str,
    path: &Path,
    file: &str,
) -> Result<Read<T>, MarsError> {
    let mut ignored = Vec::new();
    let value = serde_ignored::deserialize(toml::Deserializer::new(content), |at| {
        ignored.push(segments(&at));
    })
    .map_err(|error| ConfigError::Invalid {
        message: format!("failed to parse {}: {error}", path.display()),
    })?;

    let mut unknown_keys = Vec::new();
    for mut at in ignored {
        let Some(Segment::Key(key)) = at.pop() else {
            continue;
        };
        // `[package]` and `[plugin]` belong to the package readers, which report their own.
        if matches!(at.first(), Some(Segment::Key(table)) if table == "package" || table == "plugin")
        {
            continue;
        }
        unknown_keys.push(unknown_key_warning(file, &table_name(&at), &key));
    }
    let floor = content
        .parse::<toml::Table>()
        .ok()
        .and_then(|table| keys::floor(keys::used(&table, keys::Document::Consumer)));
    Ok(Read {
        value,
        unknown_keys,
        floor,
    })
}

pub(super) fn unknown_key_warning(file: &str, table: &str, key: &str) -> String {
    format!(
        "{file}: {}",
        mars_schema::diagnostic::unknown_key_message(table, key)
    )
}

fn segments(at: &IgnoredPath<'_>) -> Vec<Segment> {
    match at {
        IgnoredPath::Root => Vec::new(),
        IgnoredPath::Seq { parent, index } => {
            let mut out = segments(parent);
            out.push(Segment::Index(*index));
            out
        }
        IgnoredPath::Map { parent, key } => {
            let mut out = segments(parent);
            out.push(Segment::Key(key.clone()));
            out
        }
        IgnoredPath::Some { parent }
        | IgnoredPath::NewtypeStruct { parent }
        | IgnoredPath::NewtypeVariant { parent } => segments(parent),
    }
}

/// `agents.a.model-policies[0]`: the dotted table, with list positions in brackets.
pub(super) fn table_name(at: &[Segment]) -> String {
    let mut name = String::new();
    for segment in at {
        match segment {
            Segment::Key(key) => {
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(key);
            }
            Segment::Index(index) => name.push_str(&format!("[{index}]")),
        }
    }
    name
}
