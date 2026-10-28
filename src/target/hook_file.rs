//! Hook config files: a JSON file whose `hooks` object maps a native event to
//! an array of entries (Claude `settings.local.json`, Codex and Cursor
//! `hooks.json`, the plugin tree's `hooks/hooks.json`).
//!
//! Loaded, edited and saved through [`JsonDoc`], so a rewrite keeps the file's
//! symlink, permissions and key order. Hook entries are unkeyed array
//! elements, so the editing here appends and removes by equality; MCP servers,
//! keyed by name, go through `mcp_file`.

use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;
use mars_schema::ordered::OrderedValue;
use serde_json::Value;

use super::config_doc::{JsonDoc, to_ordered};
use super::{HOOK_KEY_PREFIX, HookEntry};
use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::lock::ConfigEntryRecord;

const SECTION: &str = "hooks";

fn invalid(message: String) -> MarsError {
    MarsError::Config(ConfigError::Invalid { message })
}

/// Read `path`; `None` when it does not exist. Anything but an object whose
/// `hooks` maps events to arrays is an error, so nothing is written over it.
fn load(path: &Path) -> Result<Option<JsonDoc>, MarsError> {
    let Some(doc) = JsonDoc::load(path, SECTION)? else {
        return Ok(None);
    };
    if let Some((event, _)) = doc.section().and_then(|section| {
        section
            .iter()
            .find(|(_, v)| !matches!(v, OrderedValue::Array(_)))
    }) {
        return Err(invalid(format!(
            "{}: hooks.{event} is not an array",
            path.display()
        )));
    }
    Ok(Some(doc))
}

/// Check the file at `path`, if there is one, before anything is written over it.
pub(crate) fn validate(path: &Path) -> Result<(), MarsError> {
    load(path).map(|_| ())
}

/// Append `hooks` to the file at `path`, creating it if needed, after setting
/// the top-level `top_level` keys (Cursor's `version`).
pub(super) fn write(
    path: &Path,
    hooks: &[HookEntry],
    top_level: &[(&str, Value)],
) -> Result<(), MarsError> {
    let mut doc = load(path)?.unwrap_or_else(|| JsonDoc::new(SECTION));
    for (key, value) in top_level {
        doc.set_top_level(key, value.clone());
    }
    let section = doc.section_mut();
    for hook in hooks.iter().filter(|hook| !hook.entries.is_empty()) {
        let slot = section
            .entry(hook.native_event.clone())
            .or_insert_with(|| OrderedValue::Array(Vec::new()));
        let OrderedValue::Array(events) = slot else {
            return Err(invalid(format!(
                "{}: hooks.{} is not an array",
                path.display(),
                hook.native_event
            )));
        };
        events.extend(hook.entries.iter().cloned().map(to_ordered));
    }
    doc.save(path)
}

/// Remove the entries `records` say mars wrote, wherever they still equal what
/// was recorded; an entry that has changed is left, with a warning, and the
/// package entry is appended beside it. A record without `emitted_json` names
/// nothing to match. The file is saved only if something went.
pub(super) fn remove_recorded(
    path: &Path,
    records: &BTreeMap<String, ConfigEntryRecord>,
    target: &str,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let Some(mut doc) = load(path)? else {
        return Ok(());
    };
    if doc.section().is_none() {
        return Ok(());
    }
    let mut changed = false;
    for (key, record) in records {
        let Some((event, name)) = key
            .strip_prefix(HOOK_KEY_PREFIX)
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        let Some(expected) = record
            .emitted_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<Value>>(json).ok())
        else {
            continue;
        };
        let (removed, missing) = remove_entries(doc.section_mut(), event, &expected);
        changed |= removed > 0;
        if missing > 0 {
            diag.warn(
                "config-divergence",
                format!(
                    "config-divergence: managed hook `{name}` diverged in target `{target}` at `{}`; preserving edited config and appending the package entry",
                    path.display()
                ),
            );
        }
    }
    if !changed {
        return Ok(());
    }
    doc.drop_section_if_empty();
    doc.save(path)
}

/// Remove each of `expected` from `event`'s array, dropping the event if that
/// empties it. Returns how many were removed and how many were not found.
fn remove_entries(
    section: &mut IndexMap<String, OrderedValue>,
    event: &str,
    expected: &[Value],
) -> (usize, usize) {
    let Some(OrderedValue::Array(current)) = section.get_mut(event) else {
        return (0, expected.len());
    };
    let mut removed = 0;
    for entry in expected {
        let entry = to_ordered(entry.clone());
        if let Some(index) = current.iter().position(|candidate| *candidate == entry) {
            current.remove(index);
            removed += 1;
        }
    }
    if removed > 0 && current.is_empty() {
        section.shift_remove(event);
    }
    (removed, expected.len() - removed)
}

/// Let `edit` rewrite each event's array as plain JSON values; it returns
/// whether it changed anything. An event it empties is dropped, then the
/// `hooks` object if that empties it. For the legacy sweeps that match entries
/// by command path rather than by recorded content; the file is saved only if
/// something changed.
pub(super) fn sweep_events(
    path: &Path,
    mut edit: impl FnMut(&mut Vec<Value>) -> bool,
) -> Result<(), MarsError> {
    let Some(mut doc) = load(path)? else {
        return Ok(());
    };
    let events: Vec<String> = doc
        .section()
        .map(|section| section.keys().cloned().collect())
        .unwrap_or_default();
    let mut changed = false;
    for event in events {
        let section = doc.section_mut();
        let Some(OrderedValue::Array(items)) = section.get_mut(&event) else {
            continue;
        };
        let mut values: Vec<Value> = items.iter().map(OrderedValue::to_json).collect();
        if !edit(&mut values) {
            continue;
        }
        changed = true;
        if values.is_empty() {
            section.shift_remove(&event);
        } else {
            *items = values.into_iter().map(to_ordered).collect();
        }
    }
    if !changed {
        return Ok(());
    }
    doc.drop_section_if_empty();
    doc.save(path)
}
