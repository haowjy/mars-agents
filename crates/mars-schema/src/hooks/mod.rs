//! The `hook.toml` definition, its reader and every per-target hook rule.
//!
//! A hook is written once as Claude's hooks JSON in TOML (`[hooks]`, see `portable`), a
//! native fragment per target (`[targets]`), or both. This module owns what is authored
//! (`HookDef`, `read`), which targets receive it (`HookDef::select`), and what a fragment
//! must look like (`HookDef::output`). `targets` is the per-target table and `render` turns a
//! `[hooks]` table into a profile's entries.
//! Nothing here touches the filesystem: the caller reads the bytes and places the result,
//! and adds the location (a path, or a package and version) to the path-free diagnostics.

pub mod portable;
pub mod render;
pub mod targets;
mod typescript;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;

use crate::diagnostic::{SchemaDiagnostic, Severity};
use crate::ordered::OrderedValue;
use crate::visibility::Visibility;
use portable::PortableHooks;
use render::Skip;
use targets::Target;

/// The authored placeholder for the directory a hook's files are installed in.
pub const HOOK_DIR_TOKEN: &str = "${MARS_HOOK_DIR}";

/// Hook names ending in this are reserved for files Mars generates beside authored ones.
pub(crate) const RESERVED_NAME_SUFFIX: &str = ".hooks";

pub const REMOVED_SCHEMA_MESSAGE: &str = "uses the removed v0.11.0 hook schema (`events`/`matcher`/`[action]`/`path`); \
     migrate to per-target native fragment files with `fragment = \"<target>.json\"`";

/// `Diagnostic` code of a `hook.toml` in the removed v0.11.0 schema.
pub const REMOVED_SCHEMA_CODE: &str = "hook.removed-schema";

/// Where a `hook.toml` came from. The project's own is strict. A dependency's is lenient
/// where one author's slip should not stop a sync: an unknown top-level key is ignored and
/// a merge fragment's unknown event is skipped, each with a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Dependency,
}

/// The authored placeholder for the target a hook is installed for.
pub const HOOK_TARGET_TOKEN: &str = "${MARS_HOOK_TARGET}";

/// The values a hook's text is substituted with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tokens<'a> {
    /// Replaces [`HOOK_DIR_TOKEN`]: where the hook's directory is installed.
    pub hook_dir: &'a str,
    /// Replaces [`HOOK_TARGET_TOKEN`]: the target root (`.claude`, `.codex`, …), or `plugin`
    /// in a plugin build's shared tree.
    pub hook_target: &'a str,
    /// The hook's name, which a generated file carries as its identity. Not a token: no
    /// authored text is substituted with it.
    pub hook_name: &'a str,
}

impl Tokens<'_> {
    /// `text` with every token replaced.
    pub fn substitute(&self, text: &str) -> String {
        text.replace(HOOK_DIR_TOKEN, self.hook_dir)
            .replace(HOOK_TARGET_TOKEN, self.hook_target)
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

/// One target's fragment table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The fragment file in the hook's directory: the declared one, else `<target>.json`.
    pub file: String,
    unchecked: bool,
}

impl Fragment {
    /// `unchecked = true`: events the target does not list pass through verbatim.
    pub fn is_unchecked(&self) -> bool {
        self.unchecked
    }
}

/// A `hook.toml`.
#[derive(Debug, Clone)]
pub struct HookDef {
    pub name: String,
    pub visibility: Visibility,
    pub order: i32,
    targets: BTreeMap<String, Fragment>,
    portable: Option<PortableHooks>,
    skip_targets: BTreeSet<String>,
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

    /// The `[hooks]` table, if the hook has one.
    pub fn portable(&self) -> Option<&PortableHooks> {
        self.portable.as_ref()
    }

    /// Whether `[hooks]` generates entries for `target_root`: it is not in `skip-targets`.
    fn generates_for(&self, target_root: &str) -> bool {
        self.portable.is_some() && !self.skip_targets.contains(target_root)
    }

    /// The managed target roots this hook is installed for: those where `[hooks]` renders at
    /// least one event (unless in `skip-targets`) and those with a fragment table. A fragment
    /// on a skipped target still installs; `skip-targets` only turns the generated entries
    /// off.
    pub fn select<'a>(&'a self, managed_targets: &'a [String]) -> Vec<&'a str> {
        managed_targets
            .iter()
            .map(String::as_str)
            .filter(|root| {
                self.targets.contains_key(*root)
                    || (self.generates_for(root)
                        && targets::target(root).is_some_and(|target| {
                            self.portable
                                .as_ref()
                                .is_some_and(|p| p.renders_on(&target.profile))
                        }))
            })
            .collect()
    }

    /// What `[hooks]` leaves out on `target_root` when nothing there overrides it; empty
    /// for a target in `skip-targets` and for a hook with no `[hooks]`.
    pub fn skips_for(&self, target_root: &str) -> Vec<Skip> {
        match (&self.portable, targets::target(target_root)) {
            (Some(portable), Some(target)) if self.generates_for(target_root) => {
                portable.render(&target.profile, &Tokens::default()).skips
            }
            _ => Vec::new(),
        }
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
        if unchecked.is_some() && targets::target(&root).is_some_and(Target::is_file) {
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

/// What a hook installs on one target.
#[derive(Debug, Clone, PartialEq)]
pub enum HookOutput {
    Merge(MergeOutput),
    File(FileOutput),
}

impl HookOutput {
    /// What `[hooks]` leaves out on the target, minus what a fragment replaces.
    pub fn skips(&self) -> &[Skip] {
        match self {
            Self::Merge(output) => &output.skips,
            Self::File(output) => &output.skips,
        }
    }
}

/// A file target's files for a hook: the authored fragment as written, and the file
/// generated from `[hooks]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileOutput {
    pub files: Vec<OutputFile>,
    /// What is wrong with the fragment, if anything; it is installed regardless.
    pub fragment_problem: Option<&'static str>,
    pub skips: Vec<Skip>,
    /// Rendered events whose portable answers do not all carry over to the target.
    pub answer_limits: Vec<Skip>,
}

impl FileOutput {
    /// Whether a fragment installs next to a file generated from `[hooks]`.
    pub fn has_fragment_beside_generated(&self) -> bool {
        self.files.len() > 1
    }
}

/// A file a hook installs on a file target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// Relative to the target root.
    pub path: String,
    pub content: String,
}

/// One merge target's events for a hook: `[hooks]` rendered, with the fragment's events
/// replacing the generated ones they name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MergeOutput {
    /// Finished entries per native event. An event whose array is empty is turned off.
    pub events: BTreeMap<String, Vec<OrderedValue>>,
    /// Unknown events a dependency's fragment lists, left out of `events`.
    pub skipped_events: Vec<String>,
    /// Events the target does not list that `unchecked = true` let through.
    pub unchecked_events: Vec<String>,
    /// Events the fragment declares that `[hooks]` also declares: the fragment's replace
    /// the generated entries.
    pub overrides: Vec<String>,
    /// What `[hooks]` leaves out here, minus the events a fragment replaces.
    pub skips: Vec<Skip>,
}

impl MergeOutput {
    /// Hold every event Claude lists to Codex's handler rules, reporting what that leaves
    /// out. Events outside Claude's table (`unchecked`) and events turned off stay as they
    /// are. Idempotent, so entries `[hooks]` already rendered pass unchanged.
    fn conform_to_codex(&mut self) -> Result<(), FragmentError> {
        let checked: serde_json::Map<_, _> = self
            .events
            .iter()
            .filter(|(event, entries)| {
                !entries.is_empty() && targets::claude_event(event).is_some()
            })
            .map(|(event, entries)| {
                let entries = entries.iter().map(OrderedValue::to_json).collect();
                (event.clone(), serde_json::Value::Array(entries))
            })
            .collect();
        if checked.is_empty() {
            return Ok(());
        }
        let text = serde_json::Value::Object(checked.clone()).to_string();
        let (table, nulls) = PortableHooks::from_claude_json(&text)
            .map_err(|error| FragmentError(format!("fragment is not a hook table: {error}")))?;
        for path in nulls {
            // `hooks.<Event>[..]…`: the second segment is the event.
            let event = path.split(['.', '[']).nth(1).map(str::to_owned);
            self.skips.push(render::Skip {
                event,
                reason: format!("`null` at `{path}` dropped: Codex cannot parse it"),
            });
        }
        let rendered = table.render(&targets::PLUGIN_CLAUDE_TREE, &Tokens::default());
        self.skips.extend(rendered.skips.iter().cloned());
        for event in checked.keys() {
            self.events.remove(event);
        }
        self.events.extend(rendered.events().clone());
        Ok(())
    }
}

/// Why a fragment's text is refused. The caller adds the file it came from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FragmentError(String);

impl HookDef {
    /// What this hook installs on `target`: `[hooks]` rendered (unless the target is in
    /// `skip-targets`) and the fragment laid over it. `fragment_text` is the fragment's file,
    /// read by the caller when the hook declares one for the target.
    ///
    /// On a merge target the fragment is an event-keyed JSON object, optionally wrapped as
    /// `{"hooks": …, "version"?, "description"?}`, whose events the target lists (unless
    /// its table is `unchecked`) and whose values are arrays. It replaces only the events it
    /// declares, so one with an empty array turns that event off. On a file target it is
    /// opaque text.
    pub fn output(
        &self,
        target: &Target,
        fragment_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<HookOutput, FragmentError> {
        if target.is_file() {
            Ok(HookOutput::File(self.file_output(
                target,
                fragment_text,
                tokens,
            )))
        } else {
            self.merge_output(target, &target.profile, fragment_text, origin, tokens)
                .map(HookOutput::Merge)
        }
    }

    fn file_output(
        &self,
        target: &Target,
        fragment_text: Option<&str>,
        tokens: &Tokens<'_>,
    ) -> FileOutput {
        let tokens = &Tokens {
            hook_name: &self.name,
            ..*tokens
        };
        let mut output = FileOutput::default();
        if let (Some(text), Some(path)) = (fragment_text, target.fragment_path(&self.name)) {
            let content = tokens.substitute(text);
            output.fragment_problem = file_fragment_problem(target.root, &content);
            output.files.push(OutputFile { path, content });
        }
        if let (Some(portable), true) = (&self.portable, self.generates_for(target.root)) {
            let rendered = portable.render(&target.profile, tokens);
            if let (
                render::Output::File {
                    text: Some(content),
                },
                Some(path),
            ) = (rendered.output, target.generated_path(&self.name))
            {
                output.files.push(OutputFile { path, content });
            }
            output.skips = rendered.skips;
            output.answer_limits = rendered.answer_limits;
        }
        output
    }

    /// What this hook puts in a plugin build's tree for `target`. The Claude tree's
    /// `hooks/hooks.json` is also read by Codex, which rejects the whole file over one
    /// handler it cannot parse, so everything that reaches it, `[hooks]` and fragment alike,
    /// is conformed to Codex's rules while keeping Claude's events (D21). Other trees take
    /// what [`Self::output`] gives.
    pub fn plugin_output(
        &self,
        target: &Target,
        fragment_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<HookOutput, FragmentError> {
        if target.root != ".claude" {
            return self.output(target, fragment_text, origin, tokens);
        }
        let mut output = self.merge_output(
            target,
            &targets::PLUGIN_CLAUDE_TREE,
            fragment_text,
            origin,
            tokens,
        )?;
        output.conform_to_codex()?;
        Ok(HookOutput::Merge(output))
    }

    fn merge_output(
        &self,
        target: &Target,
        profile: &targets::Profile,
        fragment_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<MergeOutput, FragmentError> {
        let mut output = MergeOutput::default();
        if let (Some(portable), true) = (&self.portable, self.generates_for(target.root)) {
            let rendered = portable.render(profile, tokens);
            if let render::Output::Merge {
                events,
                unchecked_events,
            } = rendered.output
            {
                output.events = events;
                output.unchecked_events = unchecked_events;
            }
            output.skips = rendered.skips;
        }
        let (Some(text), Some(fragment)) = (fragment_text, self.fragment(target.root)) else {
            return Ok(output);
        };
        let err = |message: String| FragmentError(message);
        let mut value: OrderedValue = serde_json::from_str(text)
            .map_err(|error| err(format!("fragment is not valid JSON: {error}")))?;
        value = portable::unwrap_hooks_object(value);
        let OrderedValue::Object(object) = value else {
            return Err(err(
                "fragment top level must be an event-keyed JSON object".into()
            ));
        };
        // Sorted, so the first fault reported does not depend on authored order.
        let sorted: BTreeMap<_, _> = object.into_iter().collect();
        let known = profile.events;
        for (event, entries) in sorted {
            if !known.iter().any(|e| e.name == event) {
                if !fragment.is_unchecked() && origin == Origin::Dependency {
                    output.skipped_events.push(event);
                    continue;
                }
                if !fragment.is_unchecked() {
                    let checked = target
                        .verified_against
                        .map_or(String::new(), |v| format!(" (checked against {v})"));
                    return Err(err(format!(
                        "unknown event `{event}` for target `{}`; valid events: {}{checked}; use `unchecked = true` to pass a newer native event through verbatim",
                        target.root,
                        known.iter().map(|e| e.name).collect::<Vec<_>>().join(", ")
                    )));
                }
                output.unchecked_events.push(event.clone());
            }
            let OrderedValue::Array(mut array) = entries else {
                return Err(err(format!("event `{event}` value must be an array")));
            };
            array.iter_mut().for_each(|e| tokens.substitute_value(e));
            if self.portable.as_ref().is_some_and(|p| p.declares(&event)) {
                output.overrides.push(event.clone());
            }
            output
                .skips
                .retain(|skip| skip.event.as_deref() != Some(event.as_str()));
            output.events.insert(event, array);
        }
        Ok(output)
    }
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
    use targets::Placement;

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
            hook_target: ".claude",
            ..Tokens::default()
        }
    }

    /// A hook with one target table per `(root, unchecked)`, and `extra` top-level text.
    fn def_with(extra: &str, tables: &[(&str, bool)]) -> HookDef {
        let mut text = format!("{extra}\n");
        for (root, unchecked) in tables {
            text.push_str(&format!("[targets.\"{root}\"]\nfragment = \"f.json\"\n"));
            if *unchecked {
                text.push_str("unchecked = true\n");
            }
        }
        read(&text, "audit", Origin::Local).def.expect(&text)
    }

    fn output(
        def: &HookDef,
        root: &str,
        fragment: Option<&str>,
        origin: Origin,
    ) -> Result<MergeOutput, FragmentError> {
        match def.output(targets::target(root).unwrap(), fragment, origin, &tokens())? {
            HookOutput::Merge(output) => Ok(output),
            HookOutput::File(_) => panic!("{root} is a file target"),
        }
    }

    fn events(root: &str) -> &'static [targets::Event] {
        let target = targets::target(root).unwrap();
        assert_eq!(target.placement, Placement::Merge, "{root}");
        target.profile.events
    }

    fn profile(root: &str) -> targets::Profile {
        targets::target(root).unwrap().profile
    }

    #[test]
    fn event_tables_match_supported_command_hook_targets() {
        let (claude, codex, cursor) = (events(".claude"), events(".codex"), events(".cursor"));
        assert_eq!(claude.len(), 29);
        assert!(claude.iter().any(|e| e.name == "SessionEnd"));
        assert_eq!(codex.len(), 11);
        assert!(codex.iter().any(|e| e.name == "SessionEnd"));
        assert_eq!(cursor.len(), 21);
        assert!(cursor.iter().any(|e| e.name == "beforeShellExecution"));
        for root in [".opencode", ".pi"] {
            assert!(targets::target(root).unwrap().is_file());
        }
        assert!(targets::target(".agents").is_none());
        // Every Codex event is a Claude event, so a Claude-keyed table renders on both.
        assert!(
            codex
                .iter()
                .all(|e| targets::claude_event(e.name).is_some())
        );
    }

    #[test]
    fn file_targets_name_their_fragment_and_generated_files() {
        let opencode = targets::target(".opencode").unwrap();
        assert_eq!(
            opencode.fragment_path("audit").as_deref(),
            Some("plugins/mars-audit.ts")
        );
        assert_eq!(
            opencode.generated_path("audit").as_deref(),
            Some("plugins/mars-audit.hooks.ts")
        );
        let pi = targets::target(".pi").unwrap();
        assert_eq!(
            pi.fragment_path("audit").as_deref(),
            Some("extensions/mars-audit.ts")
        );
        assert_eq!(
            targets::target(".codex").unwrap().fragment_path("audit"),
            None
        );
    }

    #[test]
    fn hook_dir_token_is_replaced_in_text_and_in_every_json_string() {
        let t = Tokens {
            hook_dir: "${CLAUDE_PLUGIN_ROOT}/hooks/audit",
            hook_target: "plugin",
            ..Tokens::default()
        };
        assert_eq!(
            t.substitute(r#"const SCRIPT = "${MARS_HOOK_DIR}/run.sh"; // ${MARS_HOOK_TARGET}"#),
            r#"const SCRIPT = "${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh"; // plugin"#
        );
        let def = def_with("", &[(".claude", false)]);
        let text = r#"{"Stop":[{"hooks":[{"command":"${MARS_HOOK_DIR}/run.sh"}]}]}"#;
        let merged = match def
            .output(
                targets::target(".claude").unwrap(),
                Some(text),
                Origin::Local,
                &t,
            )
            .unwrap()
        {
            HookOutput::Merge(merged) => merged,
            HookOutput::File(_) => unreachable!(),
        };
        assert_eq!(
            serde_json::to_string(&merged.events["Stop"]).unwrap(),
            r#"[{"hooks":[{"command":"${CLAUDE_PLUGIN_ROOT}/hooks/audit/run.sh"}]}]"#
        );
    }

    #[test]
    fn merge_emits_the_bytes_a_serde_json_value_would() {
        // The lock's `emitted_json` and Codex trust hash these bytes: keys sort, as they
        // did when fragments were `serde_json::Value`s.
        let def = def_with("", &[(".claude", false)]);
        let text = r#"{"version":1,"hooks":{"PreToolUse":[
            {"matcher":"Bash","hooks":[{"timeout":5,"command":"${MARS_HOOK_DIR}/run.sh","type":"command"}]}]}}"#;
        let merged = output(&def, ".claude", Some(text), Origin::Local).unwrap();
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
        let strict = def_with("", &[(".codex", false)]);
        let error = output(&strict, ".codex", Some(text), Origin::Local)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("unknown event `Bogus` for target `.codex`"));
        let loose = def_with("", &[(".codex", true)]);
        let merged = output(&loose, ".codex", Some(text), Origin::Local).unwrap();
        assert_eq!(merged.unchecked_events, ["Bogus"]);
        // A dependency's unknown event is skipped; the project's is the error above.
        let skipped = output(&strict, ".codex", Some(text), Origin::Dependency).unwrap();
        assert_eq!(skipped.skipped_events, ["Bogus"]);
        assert!(skipped.events.is_empty());
    }

    #[test]
    fn merge_refuses_non_objects_and_non_arrays() {
        let def = def_with("", &[(".claude", false)]);
        let run = |text| output(&def, ".claude", Some(text), Origin::Local);
        assert!(run("[]").is_err());
        assert!(run("nope").is_err());
        assert!(run(r#"{"Stop":{}}"#).is_err());
        // A wrapper with another key is an event table, so `hooks` is an unknown event.
        assert!(run(r#"{"hooks":{},"Stop":[]}"#).is_err());
    }

    fn read_local(text: &str) -> HookRead {
        read(text, "audit", Origin::Local)
    }

    fn read_dependency(text: &str) -> HookRead {
        read(text, "audit", Origin::Dependency)
    }

    fn message(got: &HookRead) -> &str {
        got.diagnostics[0].message()
    }

    const GUARD: &str = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "bash ${MARS_HOOK_DIR}/run.sh"

[[hooks.Notification]]
[[hooks.Notification.hooks]]
type = "command"
command = "echo n"
"#;

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
    fn hooks_select_every_managed_target_that_renders_an_event() {
        let def = read_local(GUARD).def.unwrap();
        let managed: Vec<String> = [".claude", ".codex", ".cursor", ".opencode", ".pi"]
            .map(String::from)
            .into();
        // Cursor renders nothing before 0.26; GUARD's PreToolUse renders on the two TypeScript targets.
        assert_eq!(
            def.select(&managed),
            [".claude", ".codex", ".opencode", ".pi"]
        );
        let skipped = read_local(&format!("skip-targets = [\".codex\"]\n{GUARD}"))
            .def
            .unwrap();
        assert_eq!(skipped.select(&managed), [".claude", ".opencode", ".pi"]);
        assert!(skipped.skips_for(".codex").is_empty());
        assert_eq!(def.skips_for(".codex").len(), 1);
    }

    #[test]
    fn a_fragment_still_installs_on_a_skipped_target() {
        let def = read_local(&format!(
            "skip-targets = [\".codex\"]\n{GUARD}\n[targets.\".codex\"]\nfragment = \"codex.json\"\n"
        ))
        .def
        .unwrap();
        let managed = [".codex".to_string()];
        assert_eq!(def.select(&managed), [".codex"]);
        let out = output(&def, ".codex", Some(r#"{"Stop":[]}"#), Origin::Local).unwrap();
        // Skipped: no generated PreToolUse. The fragment's own event is all there is.
        assert_eq!(out.events.keys().collect::<Vec<_>>(), ["Stop"]);
    }

    #[test]
    fn a_fragment_replaces_only_the_events_it_declares() {
        let def = read_local(&format!(
            "{GUARD}\n[targets.\".codex\"]\nfragment = \"codex.json\"\n"
        ))
        .def
        .unwrap();
        let fragment = r#"{"PreToolUse":[],"Stop":[{"hooks":[]}]}"#;
        let out = output(&def, ".codex", Some(fragment), Origin::Local).unwrap();
        // The empty array is kept, so the caller sees the event turned off.
        assert_eq!(out.events["PreToolUse"], Vec::<OrderedValue>::new());
        assert_eq!(out.overrides, ["PreToolUse"]);
        assert!(out.events.contains_key("Stop"));
        // The override check also needs a declared event: `Stop` is not in `[hooks]`.
        let untouched = output(&def, ".claude", None, Origin::Local).unwrap();
        assert_eq!(untouched.events.len(), 2);
        assert!(untouched.overrides.is_empty());
    }

    #[test]
    fn hooks_need_explicit_fragment_names() {
        let got = read_local(&format!("{GUARD}\n[targets.\".codex\"]\n"));
        assert!(got.def.is_none());
        assert!(message(&got).contains("must name its `fragment`"));
        // Without `[hooks]` an empty table keeps today's meaning.
        assert!(read_local("[targets.\".codex\"]\n").def.is_some());
    }

    #[test]
    fn a_hook_needs_hooks_or_a_target_table() {
        for text in ["", "[hooks]\n", "[targets]\n"] {
            let got = read_local(text);
            assert!(got.def.is_none(), "{text}");
            assert!(message(&got).contains("at least one target"), "{text}");
        }
    }

    #[test]
    fn unknown_events_are_strict_for_the_project_and_skipped_for_a_dependency() {
        let got = read_local(GUARD);
        assert!(got.def.is_some(), "Notification is a Claude event");
        let text = format!(
            "{GUARD}\n[[hooks.FutureEvent]]\n[[hooks.FutureEvent.hooks]]\ntype = \"command\"\ncommand = \"x\"\n"
        );
        let got = read_local(&text);
        assert!(got.def.is_none());
        assert!(message(&got).contains("unknown event `FutureEvent`"));

        let got = read_dependency(&text);
        let def = got.def.unwrap();
        assert_eq!(got.diagnostics.len(), 1);
        assert_eq!(got.diagnostics[0].code, "hook.unknown-event");
        assert!(!def.portable().unwrap().declares("FutureEvent"));
        assert!(def.portable().unwrap().declares("PreToolUse"));

        // `unchecked = true` keeps it: Claude renders it, Codex reports it.
        let def = read_local(&format!("unchecked = true\n{text}"))
            .def
            .unwrap();
        let portable = def.portable().unwrap();
        let claude = portable.render(&profile(".claude"), &tokens());
        assert_eq!(claude.unchecked_events(), ["FutureEvent"]);
        let codex = portable.render(&profile(".codex"), &tokens());
        assert!(!codex.events().contains_key("FutureEvent"));
        assert!(
            codex
                .skips
                .iter()
                .any(|s| s.event.as_deref() == Some("FutureEvent"))
        );
    }

    #[test]
    fn skip_targets_must_be_target_roots() {
        let got = read_local(&format!("skip-targets = [\"codex\"]\n{GUARD}"));
        assert!(got.def.is_none());
        assert!(message(&got).contains("not a managed target root"));
    }

    #[test]
    fn unchecked_on_a_file_target_is_an_error_for_the_project_and_a_warning_for_a_dependency() {
        let text = "[targets.\".pi\"]\nfragment = \"p.ts\"\nunchecked = false\n";
        let got = read_local(text);
        assert!(got.def.is_none());
        assert!(message(&got).contains("`unchecked` is not supported"));
        let got = read_dependency(text);
        assert!(got.def.is_some());
        assert_eq!(got.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn a_dependency_hook_fails_validation_on_a_bad_handler() {
        let got = read_dependency("[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ncommand = \"x\"\n");
        assert!(got.def.is_none());
        assert!(message(&got).contains("`type` is required"));
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
