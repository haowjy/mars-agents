//! The one reader of the consumer config files (`mars.toml`, `mars.local.toml`, the user
//! `config.toml`).
//!
//! Serde reads the keys Mars knows. Every key it leaves unread is looked up in the key table: a
//! removed key fails the load with its replacement, any other key becomes a warning and stays
//! in the file. So a new key is a struct field or a table row, and nothing here names a key.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde_ignored::Path as IgnoredPath;

use crate::error::{ConfigError, MarsError};
use mars_schema::keys::{self, Surface};

/// A file read: the value, one warning for each key Mars did not read, and the Mars its tool
/// rules need.
pub(super) struct Read<T> {
    pub(super) value: T,
    pub(super) unknown_keys: Vec<String>,
    pub(super) floor: Option<keys::Floor>,
}

thread_local! {
    static REMOVED_KEYS_TOLERATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the readers ignoring removed keys and removed values, as they ignore any key
/// they have no field for. `mars migrate` reads files that still hold them: it is the command
/// that removes them.
pub(crate) fn tolerating_removals<R>(f: impl FnOnce() -> R) -> R {
    let before = REMOVED_KEYS_TOLERATED.replace(true);
    let result = f();
    REMOVED_KEYS_TOLERATED.set(before);
    result
}

pub(super) fn removals_tolerated() -> bool {
    REMOVED_KEYS_TOLERATED.get()
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

    let mut removed = Vec::new();
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
        let table = table_name(&at);
        match surface(&at).and_then(|surface| keys::removed_key(surface, &key)) {
            Some(removal) => {
                let mut message = format!(
                    "{}: [{table}] `{key}` was removed; {}",
                    path.display(),
                    removal.advice
                );
                if let Some(detail) = removal.detail
                    && let Some(written) = written_value(content, &at, &key)
                {
                    message.push('\n');
                    message.push_str(&detail(&written));
                }
                removed.push(message);
            }
            None => unknown_keys.push(unknown_key_warning(file, &table, &key)),
        }
    }
    if !removed.is_empty() && !removals_tolerated() {
        return Err(ConfigError::Invalid {
            message: removed.join("\n"),
        }
        .into());
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

/// What the file holds at `at` + `key`, for advice that depends on it.
fn written_value(content: &str, at: &[Segment], key: &str) -> Option<toml::Value> {
    let mut value = toml::Value::Table(toml::from_str(content).ok()?);
    for segment in at {
        value = match segment {
            Segment::Key(key) => value.get(key)?.clone(),
            Segment::Index(index) => value.get(*index)?.clone(),
        };
    }
    value.get(key).cloned()
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

/// Where the key table lists the keys of this table of the file.
pub(super) fn surface(at: &[Segment]) -> Option<Surface> {
    let names: Vec<Option<&str>> = at
        .iter()
        .map(|segment| match segment {
            Segment::Key(key) => Some(key.as_str()),
            Segment::Index(_) => None,
        })
        .collect();
    match names.as_slice() {
        [Some("settings")] => Some(Surface::Settings),
        [Some("settings"), Some("meridian"), Some("agent_copy")] => Some(Surface::AgentCopy),
        [Some("agents"), Some(_)] => Some(Surface::AgentOverlay),
        [Some("agents"), Some(_), Some("tools")] => Some(Surface::AgentOverlayTools),
        [Some("skills"), Some(_)] => Some(Surface::SkillOverlay),
        _ => None,
    }
}
