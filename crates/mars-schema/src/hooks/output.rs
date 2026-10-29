//! What a hook installs on one target: `[hooks]` rendered, the native hook laid over it, and
//! the native hook's own contract checks.

use std::collections::BTreeMap;

use serde::Serialize;

use super::portable::{self, PortableHooks};
use super::render::{self, Skip};
use super::targets::{self, Target};
use super::{HookDef, Origin, Tokens};
use crate::ordered::OrderedValue;

/// What a hook installs on one target.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum HookOutput {
    Merge(MergeOutput),
    File(FileOutput),
}

impl HookOutput {
    /// What `[hooks]` leaves out on the target, minus what a native hook replaces.
    pub fn skips(&self) -> &[Skip] {
        match self {
            Self::Merge(output) => &output.skips,
            Self::File(output) => &output.skips,
        }
    }
}

/// A file target's files for a hook: the authored native hook as written, and the file
/// generated from `[hooks]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOutput {
    pub files: Vec<OutputFile>,
    /// What is wrong with the native hook, if anything; it is installed regardless.
    pub native_hook_problem: Option<&'static str>,
    pub skips: Vec<Skip>,
    /// Rendered events whose portable answers do not all carry over to the target.
    pub answer_limits: Vec<Skip>,
}

impl FileOutput {
    /// Whether a native hook installs next to a file generated from `[hooks]`.
    pub fn has_native_hook_beside_generated(&self) -> bool {
        self.files.len() > 1
    }
}

/// A file a hook installs on a file target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputFile {
    /// Relative to the target root.
    pub path: String,
    pub content: String,
}

/// One merge target's events for a hook: `[hooks]` rendered, with the native hook's events
/// replacing the generated ones they name.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutput {
    /// Finished entries per native event. An event whose array is empty is turned off.
    pub events: BTreeMap<String, Vec<OrderedValue>>,
    /// Unknown events a dependency's native hook lists, left out of `events`.
    pub skipped_events: Vec<String>,
    /// Events the target does not list that `unchecked = true` let through.
    pub unchecked_events: Vec<String>,
    /// Events the native hook declares that `[hooks]` also declares: the native hook's replace
    /// the generated entries.
    pub overrides: Vec<String>,
    /// What `[hooks]` leaves out here, minus the events a native hook replaces.
    pub skips: Vec<Skip>,
}

impl MergeOutput {
    /// Hold every event Claude lists to Codex's handler rules, reporting what that leaves
    /// out. Events outside Claude's table (`unchecked`) and events turned off stay as they
    /// are. Idempotent, so entries `[hooks]` already rendered pass unchanged.
    fn conform_to_codex(&mut self, profile: &targets::Profile) -> Result<(), NativeHookError> {
        let checked: BTreeMap<_, _> = self
            .events
            .iter()
            .filter(|(event, entries)| {
                !entries.is_empty() && targets::claude_event(event).is_some()
            })
            .map(|(event, entries)| (event.clone(), OrderedValue::Array(entries.clone())))
            .collect();
        if checked.is_empty() {
            return Ok(());
        }
        let (table, nulls) = PortableHooks::from_ordered(OrderedValue::Object(
            checked
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ))
        .map_err(|error| NativeHookError(format!("native hook is not a hook table: {error}")))?;
        for path in nulls {
            // `hooks.<Event>[..]…`: the second segment is the event.
            let event = path.split(['.', '[']).nth(1).map(str::to_owned);
            self.skips.push(render::Skip {
                event,
                reason: format!("`null` at `{path}` dropped: Codex cannot parse it"),
            });
        }
        let rendered = table.render(profile, &Tokens::default());
        self.skips.extend(rendered.skips.iter().cloned());
        for event in checked.keys() {
            self.events.remove(event);
        }
        self.events.extend(rendered.events().clone());
        Ok(())
    }
}

/// Why a native hook's text is refused. The caller adds the file it came from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NativeHookError(String);

impl HookDef {
    /// What this hook installs on `target`: `[hooks]` rendered (unless the target is in
    /// `skip-targets`) and the native hook laid over it. `native_hook_text` is the native hook's file,
    /// read by the caller when the hook declares one for the target.
    ///
    /// On a merge target the native hook is an event-keyed JSON object, optionally wrapped as
    /// `{"hooks": …, "version"?, "description"?}`, whose events the target lists (unless
    /// its table is `unchecked`) and whose values are arrays. It replaces only the events it
    /// declares, so one with an empty array turns that event off. On a file target it is
    /// opaque text.
    pub fn output(
        &self,
        target: &Target,
        native_hook_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<HookOutput, NativeHookError> {
        if target.is_file() {
            Ok(HookOutput::File(self.file_output(
                target,
                native_hook_text,
                tokens,
            )))
        } else {
            self.merge_output(target, &target.profile, native_hook_text, origin, tokens)
                .map(HookOutput::Merge)
        }
    }

    fn file_output(
        &self,
        target: &Target,
        native_hook_text: Option<&str>,
        tokens: &Tokens<'_>,
    ) -> FileOutput {
        let tokens = &Tokens {
            hook_name: &self.name,
            ..*tokens
        };
        let mut output = FileOutput::default();
        if let (Some(text), Some(path)) = (native_hook_text, target.native_hook_path(&self.name)) {
            let content = tokens.substitute(text);
            output.native_hook_problem = target.native_hook_problem(&content);
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

    /// What this hook puts in a plugin build's tree for `target`. A tree whose hook file
    /// another harness also reads passes `shared` (the Claude tree: [`targets::PLUGIN_CLAUDE_TREE`]):
    /// Codex rejects the whole `hooks/hooks.json` over one handler it cannot parse, so
    /// everything that reaches the file, `[hooks]` and native hook alike, is rendered under
    /// `shared` and conformed to Codex's rules while keeping its events (D21). Any other
    /// tree passes `None` and takes what [`Self::output`] gives.
    pub fn plugin_output(
        &self,
        target: &Target,
        shared: Option<&targets::Profile>,
        native_hook_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<HookOutput, NativeHookError> {
        let Some(profile) = shared else {
            return self.output(target, native_hook_text, origin, tokens);
        };
        let mut output = self.merge_output(target, profile, native_hook_text, origin, tokens)?;
        output.conform_to_codex(profile)?;
        Ok(HookOutput::Merge(output))
    }

    /// Whether the hook's native hook for `target` loads, as [`Self::output`] would lay it. A
    /// file target's native hook is opaque, and `[hooks]` never fails to render.
    pub fn check_native_hook(
        &self,
        target: &Target,
        native_hook_text: Option<&str>,
        origin: Origin,
    ) -> Result<(), NativeHookError> {
        if target.is_file() {
            return Ok(());
        }
        let tokens = Tokens::default();
        self.lay_native_hook(
            MergeOutput::default(),
            target,
            &target.profile,
            native_hook_text,
            origin,
            &tokens,
        )
        .map(drop)
    }

    fn merge_output(
        &self,
        target: &Target,
        profile: &targets::Profile,
        native_hook_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<MergeOutput, NativeHookError> {
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
        self.lay_native_hook(output, target, profile, native_hook_text, origin, tokens)
    }

    /// Lay the hook's native hook over the events `[hooks]` rendered.
    fn lay_native_hook(
        &self,
        mut output: MergeOutput,
        target: &Target,
        profile: &targets::Profile,
        native_hook_text: Option<&str>,
        origin: Origin,
        tokens: &Tokens<'_>,
    ) -> Result<MergeOutput, NativeHookError> {
        let (Some(text), Some(native_hook)) = (native_hook_text, self.native_hook(target.root))
        else {
            return Ok(output);
        };
        let err = |message: String| NativeHookError(message);
        let mut value: OrderedValue = serde_json::from_str(text)
            .map_err(|error| err(format!("native hook is not valid JSON: {error}")))?;
        value = portable::unwrap_hooks_object(value);
        let OrderedValue::Object(object) = value else {
            return Err(err(
                "native hook top level must be an event-keyed JSON object".into(),
            ));
        };
        // Sorted, so the first fault reported does not depend on authored order.
        let sorted: BTreeMap<_, _> = object.into_iter().collect();
        for (event, entries) in sorted {
            if profile.event(&event).is_none() {
                if !native_hook.is_unchecked() && origin == Origin::Dependency {
                    output.skipped_events.push(event);
                    continue;
                }
                if !native_hook.is_unchecked() {
                    let checked = profile
                        .verified_against
                        .map_or(String::new(), |v| format!(" (checked against {v})"));
                    return Err(err(format!(
                        "unknown event `{event}` for target `{}`; valid events: {}{checked}; use `unchecked = true` to pass a newer native event through verbatim",
                        target.root,
                        profile
                            .events
                            .iter()
                            .map(|e| e.name)
                            .collect::<Vec<_>>()
                            .join(", ")
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
