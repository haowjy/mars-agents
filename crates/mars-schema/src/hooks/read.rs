//! Reading a `hook.toml`: parse, validate by origin (D19), and build the [`HookDef`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;

use super::portable::PortableHooks;
use super::targets::{self, Target};
use super::{Fragment, HookDef, Origin, RESERVED_NAME_SUFFIX};
use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::visibility::Visibility;

pub const REMOVED_SCHEMA_MESSAGE: &str = "uses the removed v0.11.0 hook schema (`events`/`matcher`/`[action]`/`path`); \
     migrate to per-target native fragment files with `fragment = \"<target>.json\"`";

/// `Diagnostic` code of a `hook.toml` in the removed v0.11.0 schema.
pub const REMOVED_SCHEMA_CODE: &str = "hook.removed-schema";

/// What reading a `hook.toml` found. `def` is `None` when a diagnostic is an error.
#[derive(Debug, Clone)]
pub struct HookRead {
    pub def: Option<HookDef>,
    /// Path-free: the caller adds the location.
    pub diagnostics: Vec<SchemaDiagnostic>,
    /// A `visibility` that is neither `local` nor `exported`, read as `local`. The caller
    /// says so.
    pub unknown_visibility: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    #[serde(default)]
    fragment: Option<String>,
    #[serde(default)]
    unchecked: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHookDef {
    #[serde(default)]
    name: Option<String>,
    visibility: Option<String>,
    #[serde(default)]
    targets: BTreeMap<String, RawTarget>,
    #[serde(default)]
    order: i32,
    #[serde(default)]
    hooks: Option<toml::Value>,
    #[serde(default, rename = "skip-targets")]
    skip_targets: Vec<String>,
    #[serde(default)]
    unchecked: bool,
}

impl RawHookDef {
    /// The top-level keys above, for a dependency's reading to tell an unknown one.
    const KEYS: [&'static str; 7] = [
        "name",
        "visibility",
        "targets",
        "order",
        "hooks",
        "skip-targets",
        "unchecked",
    ];
}

/// Whether `hook.toml` text is in the schema Mars 0.11 removed. Text that is not TOML is
/// not.
pub fn uses_removed_schema(text: &str) -> bool {
    toml::from_str::<toml::Value>(text).is_ok_and(|value| is_removed_schema(&value))
}

fn is_removed_schema(value: &toml::Value) -> bool {
    let removed = ["events", "matcher", "action", "path"];
    let target_has_removed = value
        .get("targets")
        .and_then(toml::Value::as_table)
        .is_some_and(|targets| {
            targets.values().any(|value| {
                value
                    .as_table()
                    .is_some_and(|target| removed.iter().any(|key| target.contains_key(*key)))
            })
        });
    removed.iter().any(|key| value.get(*key).is_some())
        || target_has_removed
        || value.get("targets").is_some_and(toml::Value::is_array)
}

fn failed(code: &str, message: impl Into<String>) -> HookRead {
    HookRead {
        def: None,
        diagnostics: vec![SchemaDiagnostic::new(
            code,
            Severity::Error,
            Vec::new(),
            message,
        )],
        unknown_visibility: None,
    }
}

/// Read a `hook.toml`. `dir_name` names the hook when the file does not.
pub fn read(text: &str, dir_name: &str, origin: Origin) -> HookRead {
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(error) => return failed("hook.parse", format!("failed to parse: {error}")),
    };
    if is_removed_schema(&value) {
        return failed(REMOVED_SCHEMA_CODE, REMOVED_SCHEMA_MESSAGE);
    }
    let mut diagnostics = Vec::new();
    let warn = |diagnostics: &mut Vec<SchemaDiagnostic>, code: &str, message: String| {
        diagnostics.push(SchemaDiagnostic::new(
            code,
            Severity::Warning,
            Vec::new(),
            message,
        ));
    };
    let raw: Result<RawHookDef, _> = match origin {
        Origin::Local => toml::from_str(text),
        Origin::Dependency => {
            let mut value = value;
            if let Some(table) = value.as_table_mut() {
                let unknown: Vec<String> = table
                    .keys()
                    .filter(|key| !RawHookDef::KEYS.contains(&key.as_str()))
                    .cloned()
                    .collect();
                for key in unknown {
                    table.remove(&key);
                    warn(
                        &mut diagnostics,
                        "hook.unknown-key",
                        format!("ignoring unknown key `{key}`"),
                    );
                }
            }
            value.try_into()
        }
    };
    let raw = match raw {
        Ok(raw) => raw,
        Err(error) => return failed("hook.parse", format!("failed to parse: {error}")),
    };
    let name = raw.name.unwrap_or_else(|| dir_name.to_owned());
    if let Err(message) = validate_name(&name) {
        return failed("hook.name", format!("invalid name `{name}`: {message}"));
    }

    let mut portable = match raw.hooks.as_ref().map(PortableHooks::from_toml) {
        None => None,
        Some(Ok(portable)) => Some(portable),
        Some(Err(message)) => return failed("hook.hooks", message),
    };
    if let Some(portable) = &mut portable {
        if let Some(message) = portable.handler_problem() {
            return failed("hook.handler", message);
        }
        let unknown: Vec<String> = portable
            .events()
            .map(|(event, _)| event)
            .filter(|event| targets::claude_event(event).is_none())
            .map(str::to_owned)
            .collect();
        for event in unknown {
            if raw.unchecked {
                continue;
            }
            if origin == Origin::Local {
                return failed(
                    "hook.unknown-event",
                    format!(
                        "unknown event `{event}`; valid events: {}; set top-level `unchecked = true` to render a newer event on Claude only",
                        targets::claude_event_names()
                    ),
                );
            }
            portable.remove_event(&event);
            warn(
                &mut diagnostics,
                "hook.unknown-event",
                format!("skipping unknown event `{event}`; the hook's other events are installed"),
            );
        }
    } else if raw.unchecked {
        return failed("hook.unchecked", "top-level `unchecked` needs `[hooks]`");
    }
    if raw.targets.is_empty() && portable.as_ref().is_none_or(PortableHooks::is_empty) {
        return failed(
            "hook.no-targets",
            "needs `[hooks]` with an event, or at least one target table",
        );
    }

    let mut skip_targets = BTreeSet::new();
    for root in raw.skip_targets {
        if targets::target(&root).is_none() {
            return failed(
                "hook.skip-targets",
                format!(
                    "`skip-targets` entry `{root}` is not a managed target root such as `.codex`"
                ),
            );
        }
        skip_targets.insert(root);
    }

    let mut targets = BTreeMap::new();
    for (root, spec) in raw.targets {
        let is_file = targets::target(&root).is_some_and(Target::is_file);
        if is_file && spec.fragment.is_none() {
            return failed(
                "hook.fragment-required",
                format!(
                    "target `{root}` must name its `fragment`: a file target is a TypeScript file with no default name, and `{}.json` would be installed as one",
                    root.trim_start_matches('.')
                ),
            );
        }
        if portable.is_some() && spec.fragment.is_none() {
            return failed(
                "hook.fragment-required",
                format!(
                    "target `{root}` must name its `fragment` because the hook has `[hooks]`; an empty table would pick up a leftover `{}.json`",
                    root.trim_start_matches('.')
                ),
            );
        }
        let mut unchecked = spec.unchecked;
        if unchecked.is_some() && is_file {
            let message = format!(
                "target `{root}` uses file-mode fragments; `unchecked` is not supported because file contents and events are not validated"
            );
            if origin == Origin::Local {
                return failed("hook.unchecked-file", message);
            }
            warn(
                &mut diagnostics,
                "hook.unchecked-file",
                format!("ignoring `unchecked`: {message}"),
            );
            unchecked = None;
        }
        let file = spec
            .fragment
            .unwrap_or_else(|| format!("{}.json", root.trim_start_matches('.')));
        if let Err(message) = validate_fragment_path(&file) {
            return failed(
                "hook.fragment-path",
                format!("target `{root}` has invalid fragment `{file}`: {message}"),
            );
        }
        targets.insert(
            root,
            Fragment {
                file,
                unchecked: unchecked.unwrap_or(false),
            },
        );
    }
    let (visibility, unknown_visibility) = match raw.visibility.as_deref() {
        None => (Visibility::Local, None),
        Some(text) => match Visibility::parse(text) {
            Some(visibility) => (visibility, None),
            None => (Visibility::Local, Some(text.to_owned())),
        },
    };
    HookRead {
        def: Some(HookDef {
            name,
            visibility,
            order: raw.order,
            targets,
            portable,
            skip_targets,
        }),
        diagnostics,
        unknown_visibility,
    }
}

/// A hook's name is one directory under a target; reject anything that could escape it.
pub(super) fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.contains(['/', '\\']) {
        return Err("must be a single path component");
    }
    // The name is written into a generated file's `//` header, which U+2028 / U+2029 end.
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err("must not contain control characters");
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => return Err("must be a single path component"),
    }
    if name.ends_with(RESERVED_NAME_SUFFIX) {
        return Err("names ending in `.hooks` are reserved");
    }
    Ok(())
}

fn validate_fragment_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.contains('\0') {
        return Err("must be a non-empty relative path");
    }
    if path.contains('\\') {
        return Err("must use a relative portable path");
    }
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("must be a relative path without `.` or `..`");
    }
    Ok(())
}
