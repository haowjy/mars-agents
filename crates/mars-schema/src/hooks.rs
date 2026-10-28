//! The `hook.toml` definition, its reader and every per-target hook rule.
//!
//! A hook ships one native fragment per target it supports. This module owns what is
//! authored (`HookDef`, `read`), what each target root accepts (`placement`), and what a
//! fragment must look like (`merge_fragment`, `Fragment::reject_unchecked_for_file`).
//! Nothing here touches the filesystem: the caller reads the bytes and places the result,
//! and adds the location (a path, or a package and version) to the path-free diagnostics.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::Deserialize;

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::ordered::OrderedValue;
use crate::visibility::Visibility;

/// The authored placeholder for the directory a hook's files are installed in.
pub const HOOK_DIR_TOKEN: &str = "${MARS_HOOK_DIR}";

/// Hook names ending in this are reserved for files Mars generates beside authored ones.
const RESERVED_NAME_SUFFIX: &str = ".hooks";

pub const REMOVED_SCHEMA_MESSAGE: &str = "uses the removed v0.11.0 hook schema (`events`/`matcher`/`[action]`/`path`); \
     migrate to per-target native fragment files with `fragment = \"<target>.json\"`";

/// `Diagnostic` code of a `hook.toml` in the removed v0.11.0 schema.
pub const REMOVED_SCHEMA_CODE: &str = "hook.removed-schema";

/// Where a `hook.toml` came from. Reserved for the dependency reading to differ from the
/// project's own; both are strict today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Dependency,
}

/// The values a fragment's text is substituted with.
#[derive(Debug, Clone, Copy)]
pub struct Tokens<'a> {
    /// Replaces [`HOOK_DIR_TOKEN`]: where the hook's directory is installed.
    pub hook_dir: &'a str,
}

impl Tokens<'_> {
    /// `text` with every token replaced.
    pub fn substitute(&self, text: &str) -> String {
        text.replace(HOOK_DIR_TOKEN, self.hook_dir)
    }

    fn substitute_value(&self, value: &mut OrderedValue) {
        match value {
            OrderedValue::String(text) => *text = self.substitute(text),
            OrderedValue::Array(values) => values.iter_mut().for_each(|v| self.substitute_value(v)),
            OrderedValue::Object(values) => {
                values.values_mut().for_each(|v| self.substitute_value(v))
            }
            _ => {}
        }
    }
}

/// How a target root takes a hook fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The fragment's events are merged into the target's hook config file; `events` is
    /// every event the target documents.
    Merge { events: &'static [&'static str] },
    /// The fragment is installed as an opaque file.
    File,
}

// https://code.claude.com/docs/en/hooks — verified 2026-07-24.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "Setup",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreToolUse",
    "PermissionRequest",
    "PermissionDenied",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "Stop",
    "StopFailure",
    "TeammateIdle",
    "PreCompact",
    "PostCompact",
    "Elicitation",
    "ElicitationResult",
    "Notification",
    "ConfigChange",
    "InstructionsLoaded",
    "CwdChanged",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "SessionEnd",
];

// https://developers.openai.com/codex/hooks — verified 2026-07-24.
const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    // SessionEnd is documented at developers.openai.com/codex/hooks but was
    // runtime-verified non-firing in codex-cli 0.144.4 (2026-07-24). Re-add
    // once verified functional; authors can use `unchecked = true` meanwhile.
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

// cursor-agent 2026.07.16 installed-binary validator, extracted and
// verified by p5714 on 2026-07-24; p5695 provided the docs baseline.
const CURSOR_EVENTS: &[&str] = &[
    "beforeShellExecution",
    "beforeMCPExecution",
    "afterShellExecution",
    "afterMCPExecution",
    "beforeReadFile",
    "afterFileEdit",
    "beforeTabFileRead",
    "afterTabFileEdit",
    "stop",
    "beforeSubmitPrompt",
    "afterAgentResponse",
    "afterAgentThought",
    "sessionStart",
    "sessionEnd",
    "preCompact",
    "subagentStart",
    "subagentStop",
    "preToolUse",
    "postToolUse",
    "postToolUseFailure",
    "workspaceOpen",
];

/// How `target_root` takes hook fragments; `None` when it has no hook mechanism.
pub fn placement(target_root: &str) -> Option<Placement> {
    match target_root {
        ".claude" => Some(Placement::Merge {
            events: CLAUDE_EVENTS,
        }),
        ".codex" => Some(Placement::Merge {
            events: CODEX_EVENTS,
        }),
        ".cursor" => Some(Placement::Merge {
            events: CURSOR_EVENTS,
        }),
        ".opencode" | ".pi" => Some(Placement::File),
        _ => None,
    }
}

/// One target's fragment table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The fragment file in the hook's directory: the declared one, else `<target>.json`.
    pub file: String,
    unchecked: Option<bool>,
}

impl Fragment {
    /// `unchecked = true`: events the target does not list pass through verbatim.
    pub fn is_unchecked(&self) -> bool {
        self.unchecked.unwrap_or(false)
    }

    /// A file target installs its fragment as is, so there are no events to leave unchecked.
    pub fn reject_unchecked_for_file(&self, target_root: &str) -> Result<(), String> {
        if self.unchecked.is_some() {
            return Err(format!(
                "target `{target_root}` uses file-mode fragments; `unchecked` is not supported because file contents and events are not validated"
            ));
        }
        Ok(())
    }
}

/// A `hook.toml`.
#[derive(Debug, Clone)]
pub struct HookDef {
    pub name: String,
    pub visibility: Visibility,
    pub order: i32,
    targets: BTreeMap<String, Fragment>,
}

impl HookDef {
    /// The fragment table this hook declares for `target_root`.
    pub fn fragment(&self, target_root: &str) -> Option<&Fragment> {
        self.targets.get(target_root)
    }

    /// Every declared fragment table, by target root.
    pub fn fragments(&self) -> impl Iterator<Item = (&str, &Fragment)> {
        self.targets.iter().map(|(root, f)| (root.as_str(), f))
    }

    /// The managed target roots this hook is installed for: those with a fragment table.
    pub fn select<'a>(&'a self, managed_targets: &[String]) -> Vec<&'a str> {
        self.targets
            .keys()
            .map(String::as_str)
            .filter(|root| managed_targets.iter().any(|m| m == root))
            .collect()
    }
}

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
    targets: BTreeMap<String, RawTarget>,
    #[serde(default)]
    order: i32,
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
pub fn read(text: &str, dir_name: &str, _origin: Origin) -> HookRead {
    let value: toml::Value = match toml::from_str(text) {
        Ok(value) => value,
        Err(error) => return failed("hook.parse", format!("failed to parse: {error}")),
    };
    if is_removed_schema(&value) {
        return failed(REMOVED_SCHEMA_CODE, REMOVED_SCHEMA_MESSAGE);
    }
    let raw: RawHookDef = match toml::from_str(text) {
        Ok(raw) => raw,
        Err(error) => return failed("hook.parse", format!("failed to parse: {error}")),
    };
    if raw.targets.is_empty() {
        return failed("hook.no-targets", "at least one target table is required");
    }
    let name = raw.name.unwrap_or_else(|| dir_name.to_owned());
    if let Err(message) = validate_name(&name) {
        return failed("hook.name", format!("invalid name `{name}`: {message}"));
    }
    let mut targets = BTreeMap::new();
    for (root, spec) in raw.targets {
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
                unchecked: spec.unchecked,
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
        }),
        diagnostics: Vec::new(),
        unknown_visibility,
    }
}

/// A hook's name is one directory under a target; reject anything that could escape it.
fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("must be a single path component");
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

/// A merge fragment that passed validation, its strings substituted.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeFragment {
    pub events: BTreeMap<String, Vec<OrderedValue>>,
    /// Events the target does not list that `unchecked = true` let through.
    pub unchecked_events: Vec<String>,
}

/// Why a fragment's text is refused. The caller adds the file it came from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FragmentError(String);

/// Validate a merge-mode fragment: an event-keyed JSON object, optionally wrapped as
/// `{"hooks": …, "version"?, "description"?}`, whose events are in `known_events` (unless
/// `fragment` is unchecked) and whose values are arrays.
pub fn merge_fragment(
    text: &str,
    target_root: &str,
    known_events: &[&str],
    fragment: &Fragment,
    tokens: &Tokens<'_>,
) -> Result<MergeFragment, FragmentError> {
    let err = |message: String| FragmentError(message);
    let mut value: OrderedValue = serde_json::from_str(text)
        .map_err(|error| err(format!("fragment is not valid JSON: {error}")))?;
    if let OrderedValue::Object(object) = &mut value {
        let wrapper_keys_ok = object
            .keys()
            .all(|key| matches!(key.as_str(), "hooks" | "version" | "description"));
        if wrapper_keys_ok && let Some(inner) = object.swap_remove("hooks") {
            value = inner;
        }
    }
    let OrderedValue::Object(object) = value else {
        return Err(err(
            "fragment top level must be an event-keyed JSON object".into()
        ));
    };
    // Sorted, so the first fault reported does not depend on authored order.
    let sorted: BTreeMap<_, _> = object.into_iter().collect();
    let mut events = BTreeMap::new();
    let mut unchecked_events = Vec::new();
    for (event, entries) in sorted {
        if !known_events.contains(&event.as_str()) {
            if !fragment.is_unchecked() {
                return Err(err(format!(
                    "unknown event `{event}` for target `{target_root}`; valid events: {}; use `unchecked = true` to pass a newer native event through verbatim",
                    known_events.join(", ")
                )));
            }
            unchecked_events.push(event.clone());
        }
        let OrderedValue::Array(mut array) = entries else {
            return Err(err(format!("event `{event}` value must be an array")));
        };
        array.iter_mut().for_each(|e| tokens.substitute_value(e));
        events.insert(event, array);
    }
    Ok(MergeFragment {
        events,
        unchecked_events,
    })
}

/// Name what is wrong with a file target's fragment, if anything. Warning-only: a file
/// fragment is otherwise opaque, and the caller words the diagnostic.
///
/// `.opencode` must export a default `{ id, setup }`: `export default …`,
/// `export { x as default }` or `export * as default`. The check scans identifier tokens,
/// so a commented-out export satisfies it; the cost of that miss is a warning not shown.
pub fn file_fragment_problem(target_root: &str, content: &str) -> Option<&'static str> {
    if target_root != ".opencode" {
        return None;
    }
    let tokens: Vec<&str> = content
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$')))
        .filter(|token| !token.is_empty())
        .collect();
    let has_default_export = tokens
        .windows(2)
        .any(|pair| matches!(pair, ["export", "default"] | ["as", "default"]));
    (!has_default_export)
        .then_some("has no recognizable default export; add a default `{ id, setup }` export")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_fragments_need_a_default_export() {
        for ok in [
            "export default { id: 'a' }",
            "export { plugin as default }",
            "export * as default from './x'",
        ] {
            assert_eq!(file_fragment_problem(".opencode", ok), None, "{ok}");
        }
        assert!(file_fragment_problem(".opencode", "export const x = 1").is_some());
        assert_eq!(file_fragment_problem(".pi", "anything"), None);
    }

    fn tokens() -> Tokens<'static> {
        Tokens {
            hook_dir: "/p/.claude/hooks/audit",
        }
    }

    fn fragment(unchecked: Option<bool>) -> Fragment {
        Fragment {
            file: "claude.json".into(),
            unchecked,
        }
    }

    fn events(root: &str) -> &'static [&'static str] {
        match placement(root) {
            Some(Placement::Merge { events }) => events,
            other => panic!("{root} must merge hooks, got {other:?}"),
        }
    }

    #[test]
    fn event_allowlists_match_supported_command_hook_targets() {
        let (claude, codex, cursor) = (events(".claude"), events(".codex"), events(".cursor"));
        assert_eq!(claude.len(), 29);
        assert!(claude.contains(&"SessionEnd"));
        assert_eq!(codex.len(), 10);
        assert!(!codex.contains(&"SessionEnd"));
        assert_eq!(cursor.len(), 21);
        assert!(cursor.contains(&"beforeShellExecution"));
        assert!(cursor.contains(&"sessionStart"));
        for root in [".opencode", ".pi"] {
            assert_eq!(placement(root), Some(Placement::File));
        }
        assert_eq!(placement(".agents"), None);
    }

    #[test]
    fn hook_dir_token_is_replaced_in_text_and_in_every_json_string() {
        let t = Tokens {
            hook_dir: "${CLAUDE_PLUGIN_ROOT}/hooks/audit",
        };
        assert_eq!(
            t.substitute(r#"const SCRIPT = "${MARS_HOOK_DIR}/run.sh";"#),
            r#"const SCRIPT = "${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh";"#
        );
        let text = r#"{"Stop":[{"hooks":[{"command":"${MARS_HOOK_DIR}/run.sh"}]}]}"#;
        let merged =
            merge_fragment(text, ".claude", events(".claude"), &fragment(None), &t).unwrap();
        assert_eq!(
            serde_json::to_string(&merged.events["Stop"]).unwrap(),
            r#"[{"hooks":[{"command":"${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh"}]}]"#
        );
    }

    #[test]
    fn merge_emits_the_bytes_a_serde_json_value_would() {
        // The lock's `emitted_json` and Codex trust hash these bytes: keys sort, as they
        // did when fragments were `serde_json::Value`s.
        let text = r#"{"version":1,"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"timeout":5,"command":"${MARS_HOOK_DIR}/run.sh","type":"command"}]}]}}"#;
        let merged = merge_fragment(
            text,
            ".claude",
            events(".claude"),
            &fragment(None),
            &tokens(),
        )
        .unwrap();
        let entries: Vec<_> = merged.events["PreToolUse"]
            .iter()
            .map(OrderedValue::to_json)
            .collect();
        assert_eq!(
            serde_json::to_string(&entries).unwrap(),
            r#"[{"hooks":[{"command":"/p/.claude/hooks/audit/run.sh","timeout":5,"type":"command"}],"matcher":"Bash"}]"#
        );
    }

    #[test]
    fn merge_refuses_unknown_events_unless_unchecked() {
        let text = r#"{"Bogus":[]}"#;
        let known = events(".codex");
        let error = merge_fragment(text, ".codex", known, &fragment(None), &tokens())
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("unknown event `Bogus` for target `.codex`"));
        let merged =
            merge_fragment(text, ".codex", known, &fragment(Some(true)), &tokens()).unwrap();
        assert_eq!(merged.unchecked_events, ["Bogus"]);
    }

    #[test]
    fn merge_refuses_non_objects_and_non_arrays() {
        let known = events(".claude");
        let run = |text| merge_fragment(text, ".claude", known, &fragment(None), &tokens());
        assert!(run("[]").is_err());
        assert!(run("nope").is_err());
        assert!(run(r#"{"Stop":{}}"#).is_err());
        // A wrapper with another key is an event table, so `hooks` is an unknown event.
        assert!(run(r#"{"hooks":{},"Stop":[]}"#).is_err());
    }

    #[test]
    fn file_targets_reject_unchecked() {
        assert!(fragment(None).reject_unchecked_for_file(".pi").is_ok());
        assert!(
            fragment(Some(false))
                .reject_unchecked_for_file(".pi")
                .is_err()
        );
    }

    fn read_local(text: &str) -> HookRead {
        read(text, "audit", Origin::Local)
    }

    fn message(got: &HookRead) -> &str {
        got.diagnostics[0].message()
    }

    #[test]
    fn reads_targets_with_default_fragments() {
        let got = read_local(
            "order = 3\n[targets.\".claude\"]\n[targets.\".pi\"]\nfragment = \"x/pi.ts\"\n",
        );
        let def = got.def.unwrap();
        assert_eq!((def.name.as_str(), def.order), ("audit", 3));
        assert_eq!(def.fragment(".claude").unwrap().file, "claude.json");
        assert_eq!(def.fragment(".pi").unwrap().file, "x/pi.ts");
        assert!(def.fragment(".codex").is_none());
        let managed = [".pi".to_string(), ".codex".to_string()];
        assert_eq!(def.select(&managed), [".pi"]);
    }

    #[test]
    fn reserves_the_hooks_suffix_in_names() {
        let got = read_local("name = \"audit.hooks\"\n[targets.\".claude\"]\n");
        assert!(got.def.is_none());
        assert!(message(&got).contains("`audit.hooks`"));
        assert!(
            read("[targets.\".claude\"]\n", "dir.hooks", Origin::Local)
                .def
                .is_none()
        );
    }

    #[test]
    fn diagnostics_carry_no_path() {
        for (text, needle) in [
            ("targets = [", "failed to parse"),
            (
                "[targets.\".claude\"]\nbogus = 1\n",
                "unknown field `bogus`",
            ),
            ("[targets]\n", "at least one target"),
            (
                "[targets.\".claude\"]\nfragment = \"../x\"\n",
                "invalid fragment",
            ),
        ] {
            let got = read_local(text);
            assert!(got.def.is_none(), "{text}");
            assert!(message(&got).contains(needle), "{}", message(&got));
        }
        let removed = read_local("[[targets]]\nevent = \"x\"\n");
        assert_eq!(removed.diagnostics[0].code, REMOVED_SCHEMA_CODE);
    }

    #[test]
    fn unknown_visibility_reads_as_local() {
        let got = read_local("visibility = \"public\"\n[targets.\".claude\"]\n");
        assert_eq!(got.def.unwrap().visibility, Visibility::Local);
        assert_eq!(got.unknown_visibility.as_deref(), Some("public"));
    }
}
