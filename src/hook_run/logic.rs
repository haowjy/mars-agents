//! Claude's hook semantics, ported from `core.ts` (the TypeScript targets' shared core):
//! the matcher and `if` rules, what a finished command means, and how the handlers of an
//! event combine. Pure: the process runner is passed in. `core.ts` and this file are pinned
//! to each other by `crates/mars-schema/fixtures/hook-vectors.json`.

use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

use mars_schema::hooks::antigravity::{BLOCKING, plain_names};

/// A hook's data file, as far as the runner reads it.
#[derive(Debug, Deserialize)]
pub(super) struct Data {
    pub name: String,
    /// The hook's directory, relative to `.agy/`.
    pub dir: String,
    /// `agy` event → the Claude events it serves.
    pub serves: std::collections::BTreeMap<String, Vec<String>>,
    /// Claude event → matcher groups.
    pub events: std::collections::BTreeMap<String, Vec<Group>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Group {
    pub matcher: Option<String>,
    pub handlers: Vec<Handler>,
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Handler {
    #[serde(default)]
    pub command: String,
    pub command_windows: Option<String>,
    pub timeout: Option<f64>,
    #[serde(rename = "if")]
    pub condition: Option<String>,
    pub on_failure: Option<String>,
    #[serde(default, rename = "async")]
    pub is_async: bool,
}

/// What a finished command did.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Run {
    pub ok: bool,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Permission {
    Ask,
    Allow,
}

/// What the handlers of an event answered, in terms `reply` maps to `agy`.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Outcome {
    pub block: Option<String>,
    pub context: Option<String>,
    pub permission: Option<Permission>,
    pub updated_input: Option<Map<String, Value>>,
}

/// Claude's matcher rule: exact (`|` or `,` list) for plain names, else an unanchored regex.
pub(crate) fn matcher_accepts(pattern: Option<&str>, value: &str) -> bool {
    let Some(pattern) = pattern.filter(|p| !p.is_empty() && *p != "*") else {
        return true;
    };
    if let Some(names) = plain_names(pattern) {
        return names.contains(&value);
    }
    match regex::Regex::new(pattern) {
        Ok(regex) => regex.is_match(value),
        Err(error) => {
            eprintln!(
                "[mars hook] matcher `{pattern}` is not a regular expression this runner compiles ({error}); it matches nothing"
            );
            false
        }
    }
}

/// A superset of Claude's `if`: the tool must match, and every literal part of the glob must
/// appear in the command or path, so a script never runs less often than under Claude.
pub(crate) fn condition_holds(rule: &str, tool: &str, input: &Value) -> bool {
    let open = rule.find('(');
    let name = open.map_or(rule, |at| &rule[..at]).trim();
    if !matcher_accepts(Some(name), tool) {
        return false;
    }
    let Some(open) = open.filter(|_| rule.ends_with(')')) else {
        return true;
    };
    let field = if tool == "Bash" {
        "command"
    } else {
        "file_path"
    };
    let Some(subject) = input.get(field).and_then(Value::as_str) else {
        return true;
    };
    let haystack = if tool == "Bash" {
        subject.to_owned()
    } else {
        subject.replace('\\', "/")
    };
    let glob = &rule[open + 1..rule.len() - 1];
    let glob = glob
        .strip_suffix(":*")
        .map_or_else(|| glob.to_owned(), |g| format!("{g}*"));
    glob.split('*')
        .map(|part| {
            part.strip_prefix('~')
                .or_else(|| part.strip_prefix("./"))
                .or_else(|| part.strip_prefix('/').filter(|rest| rest.starts_with('/')))
                .unwrap_or(part)
        })
        .filter(|part| !part.is_empty())
        .all(|part| haystack.contains(part))
}

fn text_of(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The portable answers (design § What a script sees and answers) in a command's stdout;
/// any other field is ignored.
fn read_answer(event: &str, hook: &str, stdout: &str) -> Outcome {
    let text = stdout.trim();
    let mut outcome = Outcome::default();
    if !text.starts_with('{') {
        return outcome;
    }
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return outcome;
    };
    let specific = json.get("hookSpecificOutput").unwrap_or(&Value::Null);
    let blocked = |reason: Option<&str>| {
        reason.map_or_else(|| format!("Blocked by hook {hook}"), str::to_owned)
    };
    let decision = specific.get("permissionDecision").and_then(Value::as_str);
    if event == "PreToolUse" {
        match decision {
            Some("deny") => {
                outcome.block = Some(blocked(text_of(specific.get("permissionDecisionReason"))));
            }
            Some("ask") => outcome.permission = Some(Permission::Ask),
            Some("allow") => outcome.permission = Some(Permission::Allow),
            _ => {}
        }
        if outcome.permission.is_some() {
            outcome.updated_input = specific
                .get("updatedInput")
                .and_then(Value::as_object)
                .cloned();
        }
    }
    if matches!(event, "UserPromptSubmit" | "Stop")
        && json.get("decision").and_then(Value::as_str) == Some("block")
    {
        outcome.block = Some(blocked(text_of(json.get("reason"))));
    }
    if matches!(
        event,
        "PostToolUse" | "PostToolUseFailure" | "UserPromptSubmit" | "SessionStart"
    ) {
        outcome.context = text_of(specific.get("additionalContext")).map(str::to_owned);
    }
    outcome
}

/// What one finished handler means. Exit 0 parses stdout, exit 2 blocks (on an event that
/// can), any other end is ignored unless the handler asks `onFailure = "block"`.
pub(crate) fn decide_outcome(event: &str, hook: &str, handler: &Handler, run: &Run) -> Outcome {
    let blocking = BLOCKING.contains(&event);
    if !run.ok || !matches!(run.status, Some(0 | 2)) {
        if !blocking || handler.on_failure.as_deref() != Some("block") {
            return Outcome::default();
        }
        let detail = run.stderr.trim();
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Outcome {
            block: Some(format!("Blocked: hook {hook} failed ({}){detail}", run.why)),
            ..Outcome::default()
        };
    }
    if run.status == Some(2) {
        if !blocking {
            return Outcome::default();
        }
        let stderr = run.stderr.trim();
        return Outcome {
            block: Some(if stderr.is_empty() {
                format!("Blocked by hook {hook}")
            } else {
                stderr.to_owned()
            }),
            ..Outcome::default()
        };
    }
    read_answer(event, hook, &run.stdout)
}

/// One event's moment, as far as matching and running need it.
pub(super) struct Moment<'a> {
    pub cwd: &'a Path,
    /// What the groups' matchers test; `None` for an event with no matcher field.
    pub matcher: Option<&'a str>,
    /// The Claude tool name and input, for `if`; `None` off a tool event.
    pub tool: Option<(&'a str, &'a Value)>,
    /// The JSON on every handler's stdin.
    pub stdin: &'a str,
}

/// Runs the event's matching handlers in order. The first block wins over any allow or
/// ask; the context of every handler before it is joined. A handler marked `async` is
/// started and not awaited. `run` executes one handler.
pub(super) fn fire(
    data: &Data,
    event: &str,
    moment: &Moment<'_>,
    run: &mut dyn FnMut(&Handler) -> Option<Run>,
) -> Outcome {
    let mut contexts = Vec::new();
    let mut outcome = Outcome::default();
    for group in data.events.get(event).into_iter().flatten() {
        if moment
            .matcher
            .is_some_and(|value| !matcher_accepts(group.matcher.as_deref(), value))
        {
            continue;
        }
        for handler in &group.handlers {
            if let (Some(rule), Some((tool, input))) = (&handler.condition, moment.tool)
                && !condition_holds(rule, tool, input)
            {
                continue;
            }
            let Some(finished) = run(handler) else {
                continue; // started, not awaited
            };
            let mut answer = decide_outcome(event, &data.name, handler, &finished);
            if let Some(context) = answer.context.take() {
                contexts.push(context);
            }
            // A stronger permission answer wins: ask over allow.
            if answer.permission == Some(Permission::Ask)
                || (answer.permission.is_some() && outcome.permission.is_none())
            {
                outcome.permission = answer.permission;
            }
            if let Some(updated) = answer.updated_input.take() {
                outcome
                    .updated_input
                    .get_or_insert_with(Map::new)
                    .extend(updated);
            }
            if answer.block.is_some() {
                outcome.block = answer.block;
                outcome.permission = None;
                outcome.updated_input = None;
                outcome.context = (!contexts.is_empty()).then(|| contexts.join("\n"));
                return outcome;
            }
        }
    }
    outcome.context = (!contexts.is_empty()).then(|| contexts.join("\n"));
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases `core.ts` is run against too (`typescript_hooks.rs`).
    const VECTORS: &str = include_str!("../../crates/mars-schema/fixtures/hook-vectors.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).unwrap()
    }

    #[test]
    fn matcher_vectors() {
        for case in vectors()["matcher"].as_array().unwrap() {
            let pattern = case["pattern"].as_str();
            let value = case["value"].as_str().unwrap();
            assert_eq!(
                matcher_accepts(pattern, value),
                case["accepts"].as_bool().unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn condition_vectors() {
        for case in vectors()["condition"].as_array().unwrap() {
            assert_eq!(
                condition_holds(
                    case["rule"].as_str().unwrap(),
                    case["tool"].as_str().unwrap(),
                    &case["input"]
                ),
                case["holds"].as_bool().unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn decide_vectors() {
        for case in vectors()["decide"].as_array().unwrap() {
            let handler: Handler = serde_json::from_value(case["handler"].clone()).unwrap();
            let run: Run = serde_json::from_value(case["run"].clone()).unwrap();
            let outcome = decide_outcome(case["event"].as_str().unwrap(), "vec", &handler, &run);
            let expect = |key: &str| case["expect"][key].as_str().map(str::to_owned);
            assert_eq!(outcome.block, expect("block"), "{case}");
            assert_eq!(outcome.context, expect("context"), "{case}");
        }
    }

    #[test]
    fn fire_vectors() {
        for case in vectors()["fire"].as_array().unwrap() {
            let event = case["event"].as_str().unwrap();
            let events = serde_json::json!({ event: case["groups"] });
            let data: Data = serde_json::from_value(serde_json::json!({
                "name": "vec", "dir": "hooks/vec", "serves": {}, "events": events,
            }))
            .unwrap();
            let mut runs = case["runs"].as_array().unwrap().iter();
            let tool = case["tool"]
                .as_object()
                .map(|t| (t["name"].as_str().unwrap(), &t["input"]));
            let moment = Moment {
                cwd: Path::new("/p"),
                matcher: case["matcher"].as_str(),
                tool,
                stdin: "{}",
            };
            let outcome = fire(&data, event, &moment, &mut |handler| {
                if handler.is_async {
                    return None;
                }
                let mut run =
                    serde_json::json!({"ok": true, "stdout": "", "stderr": "", "why": ""});
                run.as_object_mut()
                    .unwrap()
                    .extend(runs.next().unwrap().as_object().unwrap().clone());
                if run["why"] == "" {
                    run["why"] = format!("exit {}", run["status"]).into();
                }
                Some(serde_json::from_value(run).unwrap())
            });
            assert!(
                runs.next().is_none(),
                "a scripted run was never asked for: {case}"
            );
            let expect = |key: &str| case["expect"][key].as_str().map(str::to_owned);
            assert_eq!(outcome.block, expect("block"), "{case}");
            assert_eq!(outcome.context, expect("context"), "{case}");
            if let Some(rust) = case.get("expectRust") {
                let permission = match rust["permission"].as_str() {
                    Some("ask") => Some(Permission::Ask),
                    Some("allow") => Some(Permission::Allow),
                    _ => None,
                };
                assert_eq!(outcome.permission, permission, "{case}");
                assert_eq!(
                    outcome.updated_input.map(Value::Object),
                    rust["updatedInput"]
                        .as_object()
                        .map(|o| Value::Object(o.clone())),
                    "{case}"
                );
            }
        }
    }
}
