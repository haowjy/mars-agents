//! `mars hook run`: the command a translated hook's `hooks.json` entry runs
//! (`mars_schema::hooks::agy`). It reads `agy`'s payload on stdin, builds Claude's,
//! runs the hook's matching commands with Claude's rules (`logic`) and answers in the form
//! `agy` reads for that event. Hidden: only a `hooks.json` Mars wrote names it.
//!
//! Past the data file and stdin it always exits 0 with a reply valid for the event, because
//! `agy` blocks a call on any other exit, and on a reply carrying another event's field.
//! A hook that cannot run at all (data file unreadable or of another version) is settled by
//! `--on-failure`, as the entry's guard settles a missing binary: `block` exits 1, `continue`
//! exits 0 with empty stdout.

mod exec;
mod logic;

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use mars_schema::hooks::HOOK_DIR_TOKEN;
use mars_schema::hooks::agy::{DATA_VERSION, RUNNER_HARNESS, agy_args, claude_arg, claude_tool};
use serde_json::{Map, Value, json};

use logic::{Data, Handler, Moment, Outcome, Permission, fire};

#[derive(Debug, Args)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    /// Run a translated hook for one harness event (read by `agy`, not run by hand).
    Run(RunArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OnFailure {
    Block,
    Continue,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// The harness whose payload arrives on stdin.
    #[arg(long)]
    harness: String,
    /// The harness's event.
    #[arg(long)]
    event: String,
    /// What a hook that cannot run at all does.
    #[arg(long, value_enum)]
    on_failure: OnFailure,
    /// The hook's data file, `hooks/mars-<name>.hooks.json`.
    data: PathBuf,
}

pub fn run(args: &HookArgs) -> i32 {
    let HookCommand::Run(args) = &args.command;
    match answer(args) {
        Ok(reply) => {
            if let Some(reply) = reply {
                println!("{reply}");
            }
            0
        }
        Err(message) => {
            eprintln!("mars hook run: {message}");
            match args.on_failure {
                OnFailure::Block => 1,
                OnFailure::Continue => 0,
            }
        }
    }
}

fn answer(args: &RunArgs) -> Result<Option<String>, String> {
    if args.harness != RUNNER_HARNESS {
        return Err(format!("no translation for harness `{}`", args.harness));
    }
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let path = cwd.join(&args.data);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not JSON: {error}", path.display()))?;
    let version = value.get("version").and_then(Value::as_u64);
    if version != Some(DATA_VERSION) {
        return Err(format!(
            "{} has format version {}, and this mars reads version {DATA_VERSION}; run `mars sync` with the mars that is installed",
            path.display(),
            version.map_or_else(|| "none".to_owned(), |v| v.to_string())
        ));
    }
    let mut data: Data = serde_json::from_value(value)
        .map_err(|error| format!("{} is not a hook: {error}", path.display()))?;
    // `agy` runs the command from `.agy/`, which `data.dir` is relative to.
    let hook_dir = cwd.join(&data.dir).to_string_lossy().into_owned();
    for group in data.events.values_mut().flatten() {
        for handler in &mut group.handlers {
            expand_hook_dir(handler, &hook_dir);
        }
    }
    let mut stdin = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin)
        .map_err(|error| format!("cannot read stdin: {error}"))?;
    let payload: Value = serde_json::from_str(&stdin)
        .map_err(|error| format!("stdin is not the harness's JSON: {error}"))?;
    Ok(translate(&data, &args.event, &payload))
}

fn expand_hook_dir(handler: &mut Handler, hook_dir: &str) {
    let expand = |text: &mut String| *text = text.replace(HOOK_DIR_TOKEN, hook_dir);
    expand(&mut handler.command);
    handler.command_windows.iter_mut().for_each(expand);
    handler.condition.iter_mut().for_each(expand);
}

/// `agy`'s payload for `agy_event` run through the hook, and the reply `agy` reads; `None`
/// is empty stdout.
fn translate(data: &Data, agy_event: &str, payload: &Value) -> Option<String> {
    let str_of = |key: &str| payload.get(key).and_then(Value::as_str).unwrap_or_default();
    let cwd = match str_of("cwd") {
        "" => std::env::current_dir().unwrap_or_default(),
        cwd => PathBuf::from(cwd),
    };
    let served = data
        .serves
        .get(agy_event)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let call = payload.get("toolCall").unwrap_or(&Value::Null);
    let args = call
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let tool_name = call.get("name").and_then(Value::as_str).unwrap_or_default();
    let (tool, input) = claude_tool_call(tool_name, &args, &cwd);

    let claude_event = match agy_event {
        // `agy` passes no tool output: its `error` field is what tells a failure.
        "PostToolUse" => {
            let wanted = if str_of("error").is_empty() {
                "PostToolUse"
            } else {
                "PostToolUseFailure"
            };
            served.iter().find(|event| *event == wanted)
        }
        // `agy` fires PreInvocation before every model call: the first of a prompt is 0.
        "PreInvocation" if payload.get("invocationNum").and_then(Value::as_u64) != Some(0) => None,
        _ => served.first(),
    };
    let Some(claude_event) = claude_event else {
        return reply(agy_event, &Outcome::default(), &args);
    };

    let mut fields = Map::new();
    let mut matcher = None;
    let mut tool_ref = None;
    match agy_event {
        "PreToolUse" | "PostToolUse" => {
            fields.insert("tool_name".into(), tool.clone().into());
            fields.insert("tool_input".into(), input.clone());
            fields.insert(
                "tool_use_id".into(),
                call.get("id").cloned().unwrap_or_default(),
            );
            if claude_event == "PostToolUseFailure" {
                fields.insert("error".into(), str_of("error").into());
            }
            matcher = Some(tool.as_str());
            tool_ref = Some((tool.as_str(), &input));
        }
        // `agy` fires SessionStart once per conversation and not on resume.
        "SessionStart" => {
            fields.insert("source".into(), "startup".into());
            matcher = Some("startup");
        }
        "PreInvocation" => {
            fields.insert("prompt".into(), str_of("lastUserInput").into());
        }
        "Stop" => {
            // `continuationActive` is false on every Stop, even after a `continue`; the Stop
            // hooks of one turn are counted from 0 by `executionNum`.
            let active = payload
                .get("executionNum")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > 0);
            fields.insert("stop_hook_active".into(), active.into());
        }
        _ => {}
    }
    let mut stdin = Map::new();
    stdin.insert("session_id".into(), str_of("conversationId").into());
    stdin.insert("transcript_path".into(), str_of("transcriptPath").into());
    stdin.insert("cwd".into(), cwd.to_string_lossy().into());
    stdin.insert("hook_event_name".into(), claude_event.as_str().into());
    stdin.extend(fields);
    let stdin = Value::Object(stdin).to_string();
    let moment = Moment {
        cwd: &cwd,
        matcher,
        tool: tool_ref,
        stdin: &stdin,
    };
    let outcome = fire(data, claude_event, &moment, &mut |handler| {
        exec::run(handler, moment.stdin, moment.cwd)
    });
    reply(agy_event, &outcome, &args)
}

/// Claude's tool name and input for one `agy` tool call; a tool Claude has no name for
/// keeps its own name and arguments.
fn claude_tool_call(name: &str, args: &Map<String, Value>, cwd: &Path) -> (String, Value) {
    let Some(tool) = claude_tool(name) else {
        return (name.to_owned(), Value::Object(args.clone()));
    };
    let mut input = Map::new();
    for (key, value) in args {
        input.insert(claude_arg(key).unwrap_or(key).to_owned(), value.clone());
    }
    if let Some(path) = input.get("file_path").and_then(Value::as_str)
        && Path::new(path).is_relative()
    {
        let absolute = cwd.join(path).to_string_lossy().into_owned();
        input.insert("file_path".into(), absolute.into());
    }
    (tool.to_owned(), Value::Object(input))
}

/// `updatedInput` in `agy`'s argument names: the name the call itself used, else the first
/// `agy` has been seen to take.
fn agy_overwrite(
    updated: &Map<String, Value>,
    original: &Map<String, Value>,
) -> Map<String, Value> {
    updated
        .iter()
        .map(|(key, value)| {
            let names = agy_args(key);
            let name = names
                .iter()
                .find(|name| original.contains_key(**name))
                .or_else(|| names.first())
                .map_or(key.as_str(), |name| *name);
            (name.to_owned(), value.clone())
        })
        .collect()
}

/// The reply `agy` reads on `agy_event`; `None` is empty stdout, `PreToolUse`'s "no opinion"
/// (`{}` there is a block).
fn reply(agy_event: &str, outcome: &Outcome, args: &Map<String, Value>) -> Option<String> {
    let system_message = |text: &str| json!({"systemMessage": {"systemMessage": text}});
    let reply = match agy_event {
        "PreToolUse" => match (&outcome.block, outcome.permission) {
            (Some(reason), _) => json!({"decision": "deny", "reason": reason}),
            (None, Some(permission)) => {
                let decision = match permission {
                    Permission::Ask => "ask",
                    Permission::Allow => "allow",
                };
                let mut reply = json!({"decision": decision});
                if let Some(updated) = &outcome.updated_input {
                    reply["overwrite"] = agy_overwrite(updated, args).into();
                }
                reply
            }
            (None, None) => return None,
        },
        "Stop" => match &outcome.block {
            Some(reason) => json!({"decision": "continue", "reason": reason}),
            None => json!({}),
        },
        "SessionStart" | "PreInvocation" => match (&outcome.block, &outcome.context) {
            (Some(message), _) => json!({
                "injectSteps": [{"errorMessage": {"userMessage": message}}],
                "terminationBehavior": "terminate",
            }),
            (None, Some(context)) => json!({"injectSteps": [system_message(context)]}),
            (None, None) => json!({}),
        },
        _ => json!({}),
    };
    Some(reply.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(block: Option<&str>, context: Option<&str>) -> Outcome {
        Outcome {
            block: block.map(str::to_owned),
            context: context.map(str::to_owned),
            ..Outcome::default()
        }
    }

    fn permitted(permission: Permission, updated: Option<Value>) -> Outcome {
        Outcome {
            permission: Some(permission),
            updated_input: updated.and_then(|v| v.as_object().cloned()),
            ..Outcome::default()
        }
    }

    /// One stray field on a reply blocks a tool or is read as another event's answer, so
    /// each event's reply is pinned whole.
    #[test]
    fn replies_carry_only_what_agy_reads_on_the_event() {
        let args = json!({"CommandLine": "ls", "Path": "/a"});
        let args = args.as_object().unwrap();
        let cases: Vec<(&str, Outcome, Option<Value>)> = vec![
            ("PreToolUse", Outcome::default(), None),
            (
                "PreToolUse",
                outcome(Some("no"), Some("ctx")),
                Some(json!({"decision": "deny", "reason": "no"})),
            ),
            ("PreToolUse", outcome(None, Some("ctx")), None),
            (
                "PreToolUse",
                permitted(Permission::Ask, None),
                Some(json!({"decision": "ask"})),
            ),
            (
                "PreToolUse",
                permitted(
                    Permission::Allow,
                    Some(json!({"command": "ls -l", "file_path": "/b"})),
                ),
                Some(
                    json!({"decision": "allow", "overwrite": {"CommandLine": "ls -l", "Path": "/b"}}),
                ),
            ),
            (
                "PostToolUse",
                outcome(Some("no"), Some("ctx")),
                Some(json!({})),
            ),
            ("Stop", Outcome::default(), Some(json!({}))),
            (
                "Stop",
                outcome(Some("again"), None),
                Some(json!({"decision": "continue", "reason": "again"})),
            ),
            ("SessionStart", Outcome::default(), Some(json!({}))),
            (
                "SessionStart",
                outcome(None, Some("ctx")),
                Some(json!({"injectSteps": [{"systemMessage": {"systemMessage": "ctx"}}]})),
            ),
            (
                "PreInvocation",
                outcome(Some("no"), Some("ctx")),
                Some(json!({
                    "injectSteps": [{"errorMessage": {"userMessage": "no"}}],
                    "terminationBehavior": "terminate",
                })),
            ),
            ("PreInvocation", Outcome::default(), Some(json!({}))),
        ];
        for (event, outcome, expected) in cases {
            let got = reply(event, &outcome, args)
                .map(|text| serde_json::from_str::<Value>(&text).unwrap());
            assert_eq!(got, expected, "{event} {outcome:?}");
        }
    }
}
