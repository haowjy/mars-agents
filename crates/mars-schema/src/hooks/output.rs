//! What a hook installs on one target: `[hooks]` rendered, the fragment laid over it, and
//! the fragment's own contract checks.

use std::collections::BTreeMap;

use super::portable::{self, PortableHooks};
use super::render::{self, Skip};
use super::targets::{self, Target};
use super::{HookDef, Origin, Tokens};
use crate::ordered::OrderedValue;

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
