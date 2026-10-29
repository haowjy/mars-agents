//! `[hooks]` rendered as TypeScript for the harnesses that have no hooks config: OpenCode's
//! plugins and Pi's extensions. One file per hook, `mars-<name>.hooks.ts`.
//!
//! The file is a JSON literal of the matcher groups that survive, then `core.ts` (shared:
//! Claude's tool names, matcher and `if` rules, the command runner, answer mapping), then
//! the harness's glue (`opencode.ts`, `pi.ts`: native registration and answers). The
//! template is plain text, so a command reaches the file only as a JSON string and cannot
//! break out of it.

use serde_json::{Map, Value, json};

use super::Tokens;
use super::portable::MatcherGroup;
use super::render::{Output, Rendered, Skip};
use super::targets::{self, Profile};

const CORE: &str = include_str!("typescript/core.ts");

/// One TypeScript harness's row: its glue, what its native callbacks leave out, and the
/// contract of a fragment file written for it.
#[derive(Debug, PartialEq, Eq)]
pub struct Harness {
    /// The harness's display name, in the generated file and in skip reasons.
    pub name: &'static str,
    glue: &'static str,
    /// Events the harness has no native callback for, with why.
    unsupported_events: &'static [(&'static str, &'static str)],
    /// Rendered events whose answers do not all carry over, with which.
    answer_limits: &'static [(&'static str, &'static str)],
    /// Whether a fragment must export a default `{ id, setup }` (OpenCode loads plugins so).
    fragment_needs_default_export: bool,
}

pub(super) const OPENCODE: Harness = Harness {
    name: "OpenCode",
    glue: include_str!("typescript/opencode.ts"),
    unsupported_events: &[
        (
            "SessionStart",
            "OpenCode has no per-session start hook (a plugin's setup runs once per project)",
        ),
        (
            "Stop",
            "OpenCode's end of a turn is a bus event that is not awaited",
        ),
        (
            "PostCompact",
            "OpenCode reports a finished compaction only on its bus",
        ),
        (
            "SessionEnd",
            "OpenCode reports a deleted session only on its bus",
        ),
    ],
    answer_limits: &[
        (
            "UserPromptSubmit",
            "a block answer stops the prompt, but OpenCode shows a generic error instead of the reason",
        ),
        (
            "PreCompact",
            "OpenCode cannot cancel a compaction: the script runs and a block answer is ignored",
        ),
    ],
    fragment_needs_default_export: true,
};

pub(super) const PI: Harness = Harness {
    name: "Pi",
    glue: include_str!("typescript/pi.ts"),
    unsupported_events: &[("PermissionRequest", "Pi has no permission system")],
    answer_limits: &[(
        "UserPromptSubmit",
        "a block answer stops the prompt, but Pi shows the reason only in its UI: print mode shows nothing",
    )],
    fragment_needs_default_export: false,
};

impl Harness {
    /// Name what is wrong with a fragment written for this harness, if anything.
    ///
    /// OpenCode must export a default `{ id, setup }`: `export default …`,
    /// `export { x as default }` or `export * as default`. The check scans identifier
    /// tokens, so a commented-out export satisfies it; the cost of that miss is a warning
    /// not shown.
    pub(super) fn fragment_problem(&self, content: &str) -> Option<&'static str> {
        if !self.fragment_needs_default_export {
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
}

/// `table` (the events Claude knows) as `harness` takes them, with `tokens` substituted.
pub(super) fn render(
    table: &[(&str, &[MatcherGroup])],
    profile: &Profile,
    harness: &Harness,
    tokens: &Tokens<'_>,
) -> Rendered {
    let harness_name = harness.name;
    let mut skips = Vec::new();
    let mut answer_limits = Vec::new();
    let mut events = Map::new();
    let mut skip = |event: &str, reason: String| {
        skips.push(Skip {
            event: Some(event.to_owned()),
            reason,
        })
    };
    for &(event, groups) in table {
        let claude = targets::claude_event(event).expect("render passes only Claude's events");
        let Some(row) = profile.event(event) else {
            let reason = harness
                .unsupported_events
                .iter()
                .find(|(name, _)| *name == event)
                .map_or_else(
                    || format!("{harness_name} has no such event"),
                    |(_, reason)| (*reason).to_owned(),
                );
            skip(event, reason);
            continue;
        };
        let mut kept_groups = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            let matcher = group
                .matcher
                .as_deref()
                .filter(|m| !m.is_empty() && *m != "*");
            if let (Some(matcher), Some(field), None) = (matcher, claude.matcher, row.matcher) {
                skip(
                    event,
                    format!(
                        "matcher group {g}: {harness_name} gives `{event}` no `{field}` to test `{matcher}` against"
                    ),
                );
                continue;
            }
            let mut kept = Vec::new();
            for (h, handler) in group.handlers.iter().enumerate() {
                let at = format!("matcher group {g}, handler {h}");
                let command = match (handler.kind.as_str(), handler.command.as_deref()) {
                    ("command", Some(command)) if !command.trim().is_empty() => command,
                    ("command", _) => {
                        skip(
                            event,
                            format!("{at}: a command handler needs a non-empty `command`"),
                        );
                        continue;
                    }
                    (other, _) => {
                        skip(
                            event,
                            format!("{at}: {harness_name} runs `command` handlers, not `{other}`"),
                        );
                        continue;
                    }
                };
                if handler.condition.is_some() && !claude.is_tool_event() {
                    skip(
                        event,
                        format!("{at}: Claude never runs a handler with `if` on a non-tool event"),
                    );
                    continue;
                }
                let windows = match handler.extra.get("commandWindows") {
                    Some(crate::ordered::OrderedValue::String(text)) => {
                        Some(tokens.substitute(text))
                    }
                    _ => None,
                };
                kept.push(json!({
                    "command": tokens.substitute(command),
                    "commandWindows": windows,
                    "timeout": handler.timeout,
                    "if": handler.condition.as_deref().map(|c| tokens.substitute(c)),
                    "onFailure": handler.on_failure,
                    "async": handler.is_async.unwrap_or(false),
                }));
            }
            if !kept.is_empty() {
                kept_groups.push(json!({
                    "matcher": group.matcher.as_deref().map(|m| tokens.substitute(m)),
                    "handlers": kept,
                }));
            }
        }
        if kept_groups.is_empty() {
            continue;
        }
        if let Some((_, limit)) = harness
            .answer_limits
            .iter()
            .find(|(name, _)| *name == event)
        {
            answer_limits.push(Skip {
                event: Some(event.to_owned()),
                reason: (*limit).to_owned(),
            });
        }
        events.insert(event.to_owned(), Value::Array(kept_groups));
    }
    let text = (!events.is_empty()).then(|| {
            let verified = profile.verified_against.unwrap_or(harness_name);
            let hook = json!({
                "name": tokens.hook_name,
                "harness": harness_name,
                "verified": verified,
                "events": events,
            });
            format!(
                "// Generated by mars from hook `{}`. Mars rewrites this file on every sync: edit the hook, not this file.\nconst HOOK: any = {};\n\n{CORE}{}",
                tokens.hook_name,
                serde_json::to_string_pretty(&hook).expect("a JSON value serializes"),
                harness.glue
            )
        });
    Rendered {
        output: Output::File { text },
        skips,
        answer_limits,
    }
}
