//! Hook config files: a JSON file whose `hooks` object maps a native event to
//! an array of entries (Claude `settings.local.json` and Codex
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

pub(crate) fn validate(path: &Path) -> Result<(), MarsError> {
    load(path).map(|_| ())
}

/// Append `hooks` to the file at `path`, creating it if needed.
pub(crate) fn write(path: &Path, hooks: &[HookEntry]) -> Result<(), MarsError> {
    let mut doc = load(path)?.unwrap_or_else(|| JsonDoc::new(SECTION));
    append(&mut doc, path, hooks)?;
    doc.save(path)
}

/// Remove the entries `records` say mars wrote, then append `hooks`, on one loaded document and
/// one save. A kill leaves the old file or the new one, never the file between the two, and
/// nothing else writes it between the load and the save. If the save fails the file is as it was.
pub(crate) fn replace(
    path: &Path,
    records: &BTreeMap<String, ConfigEntryRecord>,
    target: &str,
    hooks: &[HookEntry],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let mut doc = load(path)?.unwrap_or_else(|| JsonDoc::new(SECTION));
    let stripped = strip(&mut doc, path, records, target, diag);
    append(&mut doc, path, hooks)?;
    save_or_delete(&doc, path, &[], stripped)
}

/// Save `doc`, or delete the file when it holds nothing but the `ignored` keys. Only a file
/// this change emptied is deleted: one Mars never touched may be the user's, empty on purpose.
fn save_or_delete(
    doc: &JsonDoc,
    path: &Path,
    ignored: &[&str],
    changed: bool,
) -> Result<(), MarsError> {
    if !doc.is_empty_but(ignored) {
        return doc.save(path);
    }
    if changed {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn append(doc: &mut JsonDoc, path: &Path, hooks: &[HookEntry]) -> Result<(), MarsError> {
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
    Ok(())
}

/// Entries are matched one by one in order, so two hooks with identical entries need two in
/// the file.
pub(crate) fn holds_recorded(path: &Path, hooks: &[HookEntry]) -> bool {
    let Ok(Some(doc)) = load(path) else {
        return false;
    };
    let Some(section) = doc.section() else {
        return false;
    };
    let mut cursor: BTreeMap<&str, usize> = BTreeMap::new();
    for hook in hooks.iter().filter(|hook| !hook.entries.is_empty()) {
        let Some(OrderedValue::Array(current)) = section.get(&hook.native_event) else {
            return false;
        };
        let at = cursor.entry(hook.native_event.as_str()).or_default();
        for entry in &hook.entries {
            let entry = to_ordered(entry.clone());
            let Some(found) = current[*at..].iter().position(|c| *c == entry) else {
                return false;
            };
            *at += found + 1;
        }
    }
    true
}

pub(crate) type TrustPositions = BTreeMap<(String, usize, usize), Value>;

/// Where a handler sits in a hook file and what it is: `(event, matcher group, handler)` →
/// the group's matcher and the handler. Codex keys its trust on exactly this, so an entry
/// that moves or changes must be reviewed again. Only events `reads` accepts count: Codex
/// ignores any other key, so a handler there needs no review. Empty when the file is missing
/// or unreadable.
pub(crate) fn trust_positions(path: &Path, reads: impl Fn(&str) -> bool) -> TrustPositions {
    let Ok(Some(doc)) = load(path) else {
        return BTreeMap::new();
    };
    let mut positions = BTreeMap::new();
    for (event, groups) in doc.section().into_iter().flatten() {
        let OrderedValue::Array(groups) = groups else {
            continue;
        };
        if !reads(event) {
            continue;
        }
        for (g, group) in groups.iter().enumerate() {
            let group = group.to_json();
            let handlers = group.get("hooks").and_then(Value::as_array);
            for (h, handler) in handlers.into_iter().flatten().enumerate() {
                positions.insert(
                    (event.clone(), g, h),
                    serde_json::json!([group.get("matcher"), handler]),
                );
            }
        }
    }
    positions
}

/// The positions in `after` that hold something `before` did not: a new handler, or one that
/// moved or changed. A handler that only went away needs no new trust.
pub(crate) fn untrusted_positions(before: &TrustPositions, after: &TrustPositions) -> Vec<String> {
    after
        .iter()
        .filter(|(position, content)| before.get(*position) != Some(*content))
        .map(|((event, group, handler), _)| format!("{event}[{group}][{handler}]"))
        .collect()
}

/// Remove the entries `records` say mars wrote, wherever they still equal what
/// was recorded. The file is saved only if something went, and deleted if nothing else is left.
pub(crate) fn remove_recorded(
    path: &Path,
    records: &BTreeMap<String, ConfigEntryRecord>,
    target: &str,
    ignored: &[&str],
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    let Some(mut doc) = load(path)? else {
        return Ok(());
    };
    if !strip(&mut doc, path, records, target, diag) {
        return Ok(());
    }
    doc.drop_section_if_empty();
    save_or_delete(&doc, path, ignored, true)
}

/// Remove the recorded entries from `doc` in memory; whether any went. An entry that has
/// changed is left, with a warning, and the package entry is appended beside it.
fn strip(
    doc: &mut JsonDoc,
    path: &Path,
    records: &BTreeMap<String, ConfigEntryRecord>,
    target: &str,
    diag: &mut DiagnosticCollector,
) -> bool {
    if doc.section().is_none() {
        return false;
    }
    let mut changed = false;
    for (key, record) in records {
        let Some((event, name)) = key
            .strip_prefix(HOOK_KEY_PREFIX)
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        let Ok(expected) = serde_json::from_str::<Vec<Value>>(&record.emitted_json) else {
            continue;
        };
        let (removed, missing) = remove_entries(doc.section_mut(), event, &expected);
        changed |= removed > 0;
        if missing > 0 {
            diag.warn(
                "config-divergence",
                format!(
                    "config-divergence: managed hook `{name}` diverged in target `{target}` at `{}`: an entry Mars wrote was edited or deleted; any edited version stays and the package entry is appended",
                    path.display()
                ),
            );
        }
    }
    changed
}

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
        // Mars appends, so the last equal entry is usually the one it wrote. Equality ignores
        // key order, so when a user's equal entry comes later, theirs is removed and Mars's
        // stays: the count is right either way, only the surviving entry's layout differs.
        let entry = to_ordered(entry.clone());
        if let Some(index) = current.iter().rposition(|candidate| *candidate == entry) {
            current.remove(index);
            removed += 1;
        }
    }
    if removed > 0 && current.is_empty() {
        section.shift_remove(event);
    }
    (removed, expected.len() - removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn record(event: &str, command: &str) -> BTreeMap<String, ConfigEntryRecord> {
        BTreeMap::from([(
            format!("hook:{event}:audit"),
            ConfigEntryRecord {
                emitted_json: serde_json::json!([{"hooks": [{"command": command}]}]).to_string(),
            },
        )])
    }

    #[test]
    fn removal_leaves_a_diverged_file_untouched() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        let original =
            br#"{"hooks":{"SessionStart":[{"hooks":[{"command":"edited"}]}]},"keep":true}"#;
        std::fs::write(&path, original).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        remove_recorded(
            &path,
            &record("SessionStart", "original"),
            ".codex",
            &[],
            &mut DiagnosticCollector::new(),
        )
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn removal_deletes_a_file_it_empties_and_keeps_one_with_a_user_key() {
        let tmp = TempDir::new().unwrap();
        let hook = r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"original"}]}]}"#;
        for (name, user_key, left) in [
            ("owned.json", "", false),
            ("shared.json", r#","keep":1"#, true),
        ] {
            let path = tmp.path().join(name);
            std::fs::write(&path, format!("{hook}{user_key}}}")).unwrap();
            remove_recorded(
                &path,
                &record("SessionStart", "original"),
                ".claude",
                &[],
                &mut DiagnosticCollector::new(),
            )
            .unwrap();
            assert_eq!(path.exists(), left, "{name}");
        }
    }
}
