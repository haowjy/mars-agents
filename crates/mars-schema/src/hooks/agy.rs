//! `[hooks]` translated for Antigravity's `agy`, which reads `hooks.json` and runs shell
//! commands on its own events with its own payloads and replies.
//!
//! Mars writes, per hook, a data file (`hooks/mars-<name>.hooks.json`, beside the hook's
//! directory, which the file names in `dir`) holding the surviving Claude matcher groups as the
//! TypeScript targets' literal holds them and the Claude events each `agy` event serves, and
//! one `hooks.json` handler per `agy` event, which runs `mars hook run` on that file. The
//! runner (`src/hook_run`) turns `agy`'s payload into Claude's, applies
//! Claude's matcher, `if`, exit-code and answer rules, and answers in `agy`'s form. This
//! module is the rendering side; what the runner reads is [`DATA_VERSION`] of the file.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::portable::MatcherGroup;
use super::render::{self, Output, Rendered, Skip};
use super::targets::{self, Profile};
use super::typescript::{GroupRules, surviving_groups};
use super::{HOOK_DIR_TOKEN, Tokens};

/// The data file's format version. The runner treats any other version by the hook's
/// failure rule.
pub const DATA_VERSION: u64 = 1;

/// The `--harness` the runner is invoked with.
pub const RUNNER_HARNESS: &str = "agy";

/// Claude events whose answer can block the action: only these honour `onFailure = "block"`.
/// The same set as `BLOCKING` in `typescript/core.ts`.
pub const BLOCKING: &[&str] = &["PreToolUse", "UserPromptSubmit", "Stop"];

/// One `agy` event and the Claude events it serves.
#[derive(Debug, PartialEq, Eq)]
struct Mapping {
    agy: &'static str,
    claude: &'static [&'static str],
}

/// How `agy` takes `[hooks]`: the event map, what has no `agy` event, and what an event
/// maps only approximately.
#[derive(Debug, PartialEq, Eq)]
pub struct Translation {
    events: &'static [Mapping],
    unsupported_events: &'static [(&'static str, &'static str)],
    /// Rendered Claude events whose answers or firing do not all carry over, with what.
    answer_limits: &'static [(&'static str, &'static str)],
}

pub(super) const AGY: Translation = Translation {
    events: &[
        Mapping {
            agy: "PreToolUse",
            claude: &["PreToolUse"],
        },
        Mapping {
            agy: "PostToolUse",
            claude: &["PostToolUse", "PostToolUseFailure"],
        },
        Mapping {
            agy: "SessionStart",
            claude: &["SessionStart"],
        },
        Mapping {
            agy: "Stop",
            claude: &["Stop"],
        },
        Mapping {
            agy: "PreInvocation",
            claude: &["UserPromptSubmit"],
        },
    ],
    unsupported_events: &[
        (
            "SessionEnd",
            "agy has no session-end event (its Stop fires at the end of each turn)",
        ),
        (
            "PermissionRequest",
            "agy has no permission hook: a PreToolUse `ask` answer is its only way to prompt",
        ),
    ],
    answer_limits: &[
        (
            "PreToolUse",
            "an `additionalContext` answer is dropped, and `updatedInput` replaces top-level keys only (a deleted key cannot be expressed)",
        ),
        (
            "PostToolUse",
            "agy passes no tool output and reads no answer from the hook: `block` and context answers are dropped",
        ),
        (
            "PostToolUseFailure",
            "agy passes no tool output and reads no answer from the hook: `block` and context answers are dropped",
        ),
        (
            "SessionStart",
            "agy fires it once per conversation and not on resume, so a `source` matcher only ever sees `startup`",
        ),
        (
            "UserPromptSubmit",
            "agy has no prompt event: the hook runs on its PreInvocation event when that is the first of a user prompt, which recurred at each prompt and only there in a probe",
        ),
    ],
};

impl Translation {
    fn maps(&self, claude_event: &str) -> bool {
        self.events.iter().any(|m| m.claude.contains(&claude_event))
    }

    /// The Claude events `agy`'s `agy_event` serves.
    pub fn serves(&self, agy_event: &str) -> &'static [&'static str] {
        self.events
            .iter()
            .find(|m| m.agy == agy_event)
            .map_or(&[], |m| m.claude)
    }
}

/// Claude's tool names and the `agy` tools each stands for. Every `agy` name is in the
/// `agy` row of `tool_natives`, which a test pins. `Edit` takes `write_to_file` too, as the
/// `edit` capability does, but a payload's tool resolves to `Write` first ([`claude_tool`]).
const TOOLS: &[(&str, &[&str])] = &[
    ("Bash", &["run_command"]),
    ("Read", &["read_file"]),
    ("Write", &["write_to_file"]),
    (
        "Edit",
        &[
            "replace_file_content",
            "multi_replace_file_content",
            "write_to_file",
        ],
    ),
    ("Glob", &["find_by_name"]),
    ("Grep", &["grep_search"]),
    ("WebSearch", &["search_web"]),
];

/// Claude's tool input keys and the argument names `agy` has been seen to use. `Path` and
/// `CommandLine` are observed payloads (`probe-agent-frontmatter.md` §9); the others are
/// names from `agy`'s tool schemas that no probe has shown a payload for.
const ARGS: &[(&str, &[&str])] = &[
    ("command", &["CommandLine"]),
    ("file_path", &["Path", "TargetFile", "AbsolutePath"]),
    ("content", &["CodeContent"]),
    ("old_string", &["TargetContent"]),
    ("new_string", &["ReplacementContent"]),
];

/// The Claude tool name for one of `agy`'s tools; `None` for a tool Claude has no name for.
pub fn claude_tool(agy_tool: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|(_, agy)| agy.contains(&agy_tool))
        .map(|(claude, _)| *claude)
}

/// The `agy` tools a Claude tool name stands for.
pub fn agy_tools(claude_tool: &str) -> &'static [&'static str] {
    TOOLS
        .iter()
        .find(|(claude, _)| *claude == claude_tool)
        .map_or(&[], |(_, agy)| agy)
}

/// The Claude input key for an `agy` argument name.
pub fn claude_arg(agy_arg: &str) -> Option<&'static str> {
    ARGS.iter()
        .find(|(_, agy)| agy.contains(&agy_arg))
        .map(|(claude, _)| *claude)
}

/// The `agy` argument names a Claude input key goes by, most likely first.
pub fn agy_args(claude_arg: &str) -> &'static [&'static str] {
    ARGS.iter()
        .find(|(claude, _)| *claude == claude_arg)
        .map_or(&[], |(_, agy)| agy)
}

/// Claude's matcher as a list of plain names (`A|B`, `A,B`), or `None` when it is a regular
/// expression. The runner (`src/hook_run`) matches by the same rule.
pub fn plain_names(pattern: &str) -> Option<Vec<&str>> {
    (!pattern.is_empty()
        && pattern
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | ',' | '|' | '-')))
    .then(|| pattern.split(['|', ',']).map(str::trim).collect::<Vec<_>>())
}

/// Why the runner could not evaluate a Claude matcher: it compiles anything but a plain
/// name list as a Rust `regex`, which Claude's JavaScript regex is not.
fn matcher_problem(pattern: &str) -> Option<String> {
    if pattern.is_empty() || pattern == "*" || plain_names(pattern).is_some() {
        return None;
    }
    render::regex_problem(pattern)
        .err()
        .map(|reason| format!("`mars hook run` cannot compile matcher `{pattern}`: {reason}"))
}

/// The `agy` matcher for a tool event: the `agy` names of every Claude tool the groups
/// match when all of them match plain names, else `*` (the runner applies Claude's matcher
/// either way).
fn agy_matcher(claude_events: &[(&str, Vec<Value>)]) -> String {
    let mut names = Vec::new();
    for (_, groups) in claude_events {
        for group in groups {
            let Some(pattern) = group["matcher"]
                .as_str()
                .filter(|m| !m.is_empty() && *m != "*")
            else {
                return "*".into();
            };
            let Some(plain) = plain_names(pattern) else {
                return "*".into();
            };
            for name in plain {
                let tools = agy_tools(name);
                if tools.is_empty() {
                    return "*".into();
                }
                names.extend(tools.iter().copied());
            }
        }
    }
    names.sort_unstable();
    names.dedup();
    names.join("|")
}

/// The seconds `agy` waits for the runner: the runner runs a group's handlers in sequence,
/// each up to its own timeout (an `async` handler none), and `agy` must outlast the last
/// kill so the runner can answer.
fn runner_timeout(claude_events: &[(&str, Vec<Value>)]) -> u64 {
    let total: f64 = claude_events
        .iter()
        .flat_map(|(_, groups)| groups)
        .flat_map(|group| group["handlers"].as_array().into_iter().flatten())
        .filter_map(|handler| handler["timeout"].as_f64())
        .sum();
    total.ceil() as u64 + 1
}

/// Whether a handler of the served events asks to block when the hook itself fails.
fn blocks_on_failure(claude_events: &[(&str, Vec<Value>)]) -> bool {
    claude_events
        .iter()
        .filter(|(event, _)| BLOCKING.contains(event))
        .flat_map(|(_, groups)| groups)
        .flat_map(|group| group["handlers"].as_array().into_iter().flatten())
        .any(|handler| handler["onFailure"] == "block")
}

/// The command `agy` runs for one event: the `mars` that ran the sync, behind a guard that
/// settles a missing binary by the hook's failure rule (exit 1 blocks the call, exit 0 with
/// no output leaves it alone). Without the guard a missing binary exits 127, which `agy`
/// reads as a block. `data` is relative to `.agy/`, where `agy` runs it.
pub fn runner_command(
    exe: &str,
    agy_event: &str,
    data: &str,
    block: bool,
    windows: bool,
) -> String {
    let on_failure = if block { "block" } else { "continue" };
    let args = format!(
        "hook run --harness {RUNNER_HARNESS} --event {agy_event} --on-failure {on_failure}"
    );
    if windows {
        // `agy` runs `cmd /c` there. Untested: no Windows machine has run it.
        let data = data.replace('/', "\\");
        format!(
            "if exist \"{exe}\" (\"{exe}\" {args} \"{data}\") else (exit /b {})",
            u8::from(block)
        )
    } else {
        let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
        format!(
            "m={}; [ -x \"$m\" ] || {{ echo \"mars hook: $m is gone; run mars sync\" >&2; exit {}; }}; exec \"$m\" {args} {}",
            quote(exe),
            u8::from(block),
            quote(data)
        )
    }
}

/// `table` (the events Claude knows) translated for `agy`, with `tokens` substituted.
/// `${MARS_HOOK_DIR}` stays in the data file: the runner resolves it from the file's `dir`
/// (the hook's directory relative to `.agy/`), so the file holds no machine path.
pub(super) fn render(
    table: &[(&str, &[MatcherGroup])],
    profile: &Profile,
    translation: &Translation,
    tokens: &Tokens<'_>,
    windows: bool,
) -> Rendered {
    let mut skips = Vec::new();
    let mut answer_limits = Vec::new();
    let mut skip = |event: &str, reason: String| {
        skips.push(Skip {
            event: Some(event.to_owned()),
            reason,
        })
    };
    let deferred = Tokens {
        hook_dir: HOOK_DIR_TOKEN,
        ..*tokens
    };
    for &(event, _) in table {
        if !translation.maps(event) {
            let reason = translation
                .unsupported_events
                .iter()
                .find(|(name, _)| *name == event)
                .map_or_else(
                    || "agy has no such event".to_owned(),
                    |(_, reason)| (*reason).to_owned(),
                );
            skip(event, reason);
        }
    }
    let mut entries = BTreeMap::new();
    let mut serves = serde_json::Map::new();
    let mut by_claude = serde_json::Map::new();
    for mapping in translation.events {
        let row = profile
            .event(mapping.agy)
            .expect("every mapped agy event has a row");
        let mut kept: Vec<(&str, Vec<Value>)> = Vec::new();
        for &claude_name in mapping.claude {
            let Some((_, groups)) = table.iter().find(|(event, _)| *event == claude_name) else {
                continue;
            };
            let claude = targets::claude_event(claude_name).expect("Claude's event");
            let groups = surviving_groups(
                claude_name,
                groups,
                claude,
                &GroupRules {
                    harness_name: "agy",
                    supplies_matcher: row.matcher.is_some(),
                    tokens: &deferred,
                    matcher_problem: &matcher_problem,
                },
                &mut skip,
            );
            if groups.is_empty() {
                continue;
            }
            if let Some((_, limit)) = translation
                .answer_limits
                .iter()
                .find(|(name, _)| *name == claude_name)
            {
                answer_limits.push(Skip {
                    event: Some(claude_name.to_owned()),
                    reason: (*limit).to_owned(),
                });
            }
            kept.push((claude_name, groups));
        }
        if kept.is_empty() {
            continue;
        }
        let command = runner_command(
            tokens.mars_exe,
            mapping.agy,
            tokens.generated_file,
            blocks_on_failure(&kept),
            windows,
        );
        let handler = json!({
            "type": "command",
            "command": command,
            "timeout": runner_timeout(&kept),
        });
        let entry = if row.is_tool_event() {
            json!({"matcher": agy_matcher(&kept), "hooks": [handler]})
        } else {
            handler
        };
        entries.insert(
            mapping.agy.to_owned(),
            vec![serde_json::from_value(entry).expect("a JSON value is an ordered value")],
        );
        serves.insert(
            mapping.agy.to_owned(),
            kept.iter().map(|(event, _)| *event).collect(),
        );
        for (claude_name, groups) in kept {
            by_claude.insert(claude_name.to_owned(), Value::Array(groups));
        }
    }
    let data_file = (!entries.is_empty()).then(|| {
        let data = json!({
            "version": DATA_VERSION,
            "name": tokens.hook_name,
            "dir": tokens.hook_dir,
            "verified": profile.verified_against,
            "serves": serves,
            "events": by_claude,
        });
        serde_json::to_string_pretty(&data).expect("a JSON value serializes") + "\n"
    });
    Rendered {
        output: Output::Keyed {
            events: entries,
            data_file,
        },
        skips,
        answer_limits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessId;
    use crate::tool_natives::lookup;

    /// Every `agy` tool the table names is one the `agy` row of `tool_natives` knows, so a
    /// tool renamed there cannot silently stop matching here.
    #[test]
    fn every_agy_tool_is_in_the_native_tool_table() {
        let known: Vec<&str> = crate::tool_names::canonical_capabilities()
            .filter_map(|capability| lookup(capability, HarnessId::Agy))
            .flat_map(|native| native.native_tools().iter().copied())
            .collect();
        for (claude, tools) in TOOLS {
            for tool in *tools {
                assert!(
                    known.contains(tool),
                    "{claude}: `{tool}` is not an agy tool"
                );
            }
        }
    }
}
