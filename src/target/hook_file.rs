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
pub(crate) fn write(
    path: &Path,
    hooks: &[HookEntry],
    top_level: &[(&str, i64)],
) -> Result<(), MarsError> {
    let mut doc = load(path)?.unwrap_or_else(|| JsonDoc::new(SECTION));
    for (key, value) in top_level {
        doc.set_top_level(key, Value::from(*value));
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

/// Whether the file at `path` already holds `hooks` as a sync would write them: per event,
/// every entry present, in `hooks`' order. Other entries may sit between them. A sync that
/// changes nothing and finds this true leaves the file alone; a file that lost an entry, or
/// holds them in another order, is rewritten.
///
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

/// `(event, matcher group, handler)` → what sits there; see [`trust_positions`].
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
/// was recorded; an entry that has changed is left, with a warning, and the
/// package entry is appended beside it. The file is saved only if something went.
pub(crate) fn remove_recorded(
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(name: &str, event: &str, script: &str) -> HookEntry {
        HookEntry {
            name: name.to_string(),
            native_event: event.to_string(),
            entries: vec![
                serde_json::json!({"hooks": [{"type": "command", "command": format!("bash '{script}'")}]}),
            ],
        }
    }

    fn record(event: &str, command: &str) -> BTreeMap<String, ConfigEntryRecord> {
        BTreeMap::from([(
            format!("hook:{event}:audit"),
            ConfigEntryRecord {
                emitted_json: serde_json::json!([{"hooks": [{"command": command}]}]).to_string(),
            },
        )])
    }

    #[test]
    fn write_creates_the_file_and_appends_in_call_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        write(&path, &[entry("audit", "PreToolUse", "/old/run.sh")], &[]).unwrap();
        write(&path, &[entry("audit", "PreToolUse", "/new/run.sh")], &[]).unwrap();

        let json: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let commands: Vec<_> = json["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert_eq!(commands, ["bash '/old/run.sh'", "bash '/new/run.sh'"]);
    }

    /// An entry edited since mars wrote it is not removed, and the file is not rewritten at all.
    #[test]
    fn removal_leaves_a_diverged_file_untouched() {
        // (target, file contents, recorded entries for `hook:<event>:audit`)
        let cases = [
            (
                ".claude",
                br#"{"hooks":{"SessionStart":[{"hooks":[{"command":"edited"}]}]},"keep":true}"#
                    .as_slice(),
                serde_json::json!([{"hooks":[{"command":"original"}]}]),
            ),
            (
                ".cursor",
                br#"{"version":1,"hooks":{"SessionStart":[{"command":"edited"}]},"keep":true}"#
                    .as_slice(),
                serde_json::json!([{"command":"original"}]),
            ),
        ];
        for (target, original, recorded) in cases {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("hooks.json");
            std::fs::write(&path, original).unwrap();
            // Pin the mtime in the past so a rewrite is visible without sleeping.
            let pinned = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(pinned)
                .unwrap();
            let records = BTreeMap::from([(
                "hook:SessionStart:audit".to_string(),
                ConfigEntryRecord {
                    emitted_json: recorded.to_string(),
                },
            )]);

            remove_recorded(&path, &records, target, &mut DiagnosticCollector::new()).unwrap();

            assert_eq!(std::fs::read(&path).unwrap(), original, "{target}");
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                pinned,
                "{target}"
            );
        }
    }

    #[test]
    fn removal_prunes_an_emptied_hooks_object() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hooks.json");
        std::fs::write(
            &path,
            serde_json::json!({"version": 1, "hooks": {"sessionStart": [{"hooks": [{"command": "owned"}]}]}})
                .to_string(),
        )
        .unwrap();

        remove_recorded(
            &path,
            &record("sessionStart", "owned"),
            ".cursor",
            &mut DiagnosticCollector::new(),
        )
        .unwrap();

        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, serde_json::json!({"version": 1}));
    }
}
