//! The portable `[hooks]` table: Claude's `"hooks"` object, written as TOML.
//!
//! `[[hooks.<Event>]]` is a matcher group and `[[hooks.<Event>.hooks]]` a handler, with
//! Claude's names spelled exactly. The fields mars acts on are typed; every other handler
//! and group field is kept and travels to Claude unchanged. Claude JSON and this TOML
//! convert into each other; JSON `null`, which TOML cannot hold, is dropped and reported.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde_json::Number;

use crate::ordered::OrderedValue;

/// One handler. Claude's `type` is [`Self::kind`] because `type` is a Rust keyword.
#[derive(Debug, Clone, PartialEq)]
pub struct Handler {
    pub kind: String,
    pub command: Option<String>,
    pub timeout: Option<Number>,
    pub condition: Option<String>,
    pub on_failure: Option<String>,
    pub status_message: Option<String>,
    pub is_async: Option<bool>,
    /// Every other field, in authored order.
    pub extra: IndexMap<String, OrderedValue>,
}

/// One matcher group: a `matcher` and the handlers it guards.
#[derive(Debug, Clone, PartialEq)]
pub struct MatcherGroup {
    pub matcher: Option<String>,
    pub handlers: Vec<Handler>,
    pub extra: IndexMap<String, OrderedValue>,
}

/// A hook's `[hooks]` table: the matcher groups of each event.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PortableHooks {
    events: BTreeMap<String, Vec<MatcherGroup>>,
}

fn object(value: OrderedValue, at: &str) -> Result<IndexMap<String, OrderedValue>, String> {
    match value {
        OrderedValue::Object(map) => Ok(map),
        _ => Err(format!("{at} must be a table")),
    }
}

fn take_string(
    map: &mut IndexMap<String, OrderedValue>,
    key: &str,
    at: &str,
) -> Result<Option<String>, String> {
    match map.shift_remove(key) {
        None => Ok(None),
        Some(OrderedValue::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("{at}: `{key}` must be a string")),
    }
}

impl Handler {
    fn from_value(value: OrderedValue, at: &str) -> Result<Self, String> {
        let mut map = object(value, at)?;
        let kind = take_string(&mut map, "type", at)?
            .ok_or_else(|| format!("{at}: `type` is required"))?;
        let timeout = match map.shift_remove("timeout") {
            None => None,
            Some(OrderedValue::Number(number)) => Some(number),
            Some(_) => return Err(format!("{at}: `timeout` must be a number")),
        };
        let is_async = match map.shift_remove("async") {
            None => None,
            Some(OrderedValue::Bool(flag)) => Some(flag),
            Some(_) => return Err(format!("{at}: `async` must be a boolean")),
        };
        Ok(Self {
            command: take_string(&mut map, "command", at)?,
            condition: take_string(&mut map, "if", at)?,
            on_failure: take_string(&mut map, "onFailure", at)?,
            status_message: take_string(&mut map, "statusMessage", at)?,
            kind,
            timeout,
            is_async,
            extra: map,
        })
    }

    pub(super) fn to_value(&self) -> OrderedValue {
        let mut map = IndexMap::new();
        let mut put = |key: &str, value: Option<OrderedValue>| {
            if let Some(value) = value {
                map.insert(key.to_owned(), value);
            }
        };
        put("type", Some(OrderedValue::String(self.kind.clone())));
        put("command", self.command.clone().map(OrderedValue::String));
        put("timeout", self.timeout.clone().map(OrderedValue::Number));
        put("if", self.condition.clone().map(OrderedValue::String));
        put(
            "onFailure",
            self.on_failure.clone().map(OrderedValue::String),
        );
        put(
            "statusMessage",
            self.status_message.clone().map(OrderedValue::String),
        );
        put("async", self.is_async.map(OrderedValue::Bool));
        map.extend(self.extra.clone());
        OrderedValue::Object(map)
    }
}

impl MatcherGroup {
    fn from_value(value: OrderedValue, at: &str) -> Result<Self, String> {
        let mut map = object(value, at)?;
        let matcher = take_string(&mut map, "matcher", at)?;
        let handlers = match map.shift_remove("hooks") {
            Some(OrderedValue::Array(handlers)) => handlers
                .into_iter()
                .enumerate()
                .map(|(i, handler)| Handler::from_value(handler, &format!("{at}.hooks[{i}]")))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(format!("{at}: `hooks` must be an array of tables")),
            None => return Err(format!("{at}: `hooks` is required")),
        };
        Ok(Self {
            matcher,
            handlers,
            extra: map,
        })
    }

    pub(super) fn to_value(&self, handlers: &[Handler]) -> OrderedValue {
        let mut map = IndexMap::new();
        if let Some(matcher) = &self.matcher {
            map.insert("matcher".to_owned(), OrderedValue::String(matcher.clone()));
        }
        map.insert(
            "hooks".to_owned(),
            OrderedValue::Array(handlers.iter().map(Handler::to_value).collect()),
        );
        map.extend(self.extra.clone());
        OrderedValue::Object(map)
    }

    /// This group as Claude's JSON entry.
    pub fn to_claude(&self) -> OrderedValue {
        self.to_value(&self.handlers)
    }
}

/// A TOML value as the JSON-shaped value. A datetime becomes its text, as JSON has none.
fn from_toml(value: &toml::Value) -> OrderedValue {
    match value {
        toml::Value::String(text) => OrderedValue::String(text.clone()),
        toml::Value::Integer(number) => OrderedValue::Number((*number).into()),
        toml::Value::Float(number) => Number::from_f64(*number).map_or_else(
            || OrderedValue::String(number.to_string()),
            OrderedValue::Number,
        ),
        toml::Value::Boolean(flag) => OrderedValue::Bool(*flag),
        toml::Value::Datetime(date) => OrderedValue::String(date.to_string()),
        toml::Value::Array(values) => OrderedValue::Array(values.iter().map(from_toml).collect()),
        toml::Value::Table(table) => OrderedValue::Object(
            table
                .iter()
                .map(|(key, value)| (key.clone(), from_toml(value)))
                .collect(),
        ),
    }
}

/// An ordered value as TOML. `null` never reaches here: the model holds none.
fn to_toml_value(value: &OrderedValue) -> toml::Value {
    match value {
        OrderedValue::Null => unreachable!("a portable hook holds no null"),
        OrderedValue::Bool(flag) => toml::Value::Boolean(*flag),
        OrderedValue::Number(number) => number
            .as_i64()
            .map(toml::Value::Integer)
            .or_else(|| number.as_f64().map(toml::Value::Float))
            .expect("a JSON number is an integer or a float"),
        OrderedValue::String(text) => toml::Value::String(text.clone()),
        OrderedValue::Array(values) => {
            toml::Value::Array(values.iter().map(to_toml_value).collect())
        }
        OrderedValue::Object(map) => toml::Value::Table(
            map.iter()
                .map(|(key, value)| (key.clone(), to_toml_value(value)))
                .collect(),
        ),
    }
}

/// Remove every `null` (an array element, or an object key), saying where each was.
fn drop_nulls(value: &mut OrderedValue, at: &str, dropped: &mut Vec<String>) {
    match value {
        OrderedValue::Array(values) => {
            let mut index = 0;
            values.retain_mut(|value| {
                let here = format!("{at}[{index}]");
                index += 1;
                if *value == OrderedValue::Null {
                    dropped.push(here);
                    return false;
                }
                drop_nulls(value, &here, dropped);
                true
            });
        }
        OrderedValue::Object(map) => {
            map.retain(|key, value| {
                let here = format!("{at}.{key}");
                if *value == OrderedValue::Null {
                    dropped.push(here);
                    return false;
                }
                drop_nulls(value, &here, dropped);
                true
            });
        }
        _ => {}
    }
}

/// `{"hooks": …}` (a settings file or plugin `hooks.json`, with only `hooks`, `version` and
/// `description` beside it) is its inner object; any other object is itself an event table.
pub(super) fn unwrap_hooks_object(value: OrderedValue) -> OrderedValue {
    if let OrderedValue::Object(mut object) = value {
        let wrapper = object
            .keys()
            .all(|key| matches!(key.as_str(), "hooks" | "version" | "description"));
        if wrapper && let Some(inner) = object.shift_remove("hooks") {
            return inner;
        }
        return OrderedValue::Object(object);
    }
    value
}

impl PortableHooks {
    fn from_ordered(value: OrderedValue) -> Result<Self, String> {
        let mut events = BTreeMap::new();
        for (event, groups) in object(value, "`hooks`")? {
            let OrderedValue::Array(groups) = groups else {
                return Err(format!("`hooks.{event}` must be an array of tables"));
            };
            let groups = groups
                .into_iter()
                .enumerate()
                .map(|(i, group)| MatcherGroup::from_value(group, &format!("`hooks.{event}[{i}]`")))
                .collect::<Result<Vec<_>, _>>()?;
            events.insert(event, groups);
        }
        Ok(Self { events })
    }

    /// The `[hooks]` table of a `hook.toml`.
    pub fn from_toml(value: &toml::Value) -> Result<Self, String> {
        Self::from_ordered(from_toml(value))
    }

    /// Claude's hooks JSON: an event table, or a settings file or plugin `hooks.json`
    /// wrapping one. TOML cannot hold `null`, so each is dropped; the second value says
    /// where.
    pub fn from_claude_json(text: &str) -> Result<(Self, Vec<String>), String> {
        let mut value: OrderedValue =
            serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
        let mut dropped = Vec::new();
        drop_nulls(&mut value, "hooks", &mut dropped);
        Ok((Self::from_ordered(unwrap_hooks_object(value))?, dropped))
    }

    /// Claude's event table: what Claude reads under `"hooks"`.
    pub fn to_claude_json(&self) -> OrderedValue {
        OrderedValue::Object(
            self.events
                .iter()
                .map(|(event, groups)| {
                    (
                        event.clone(),
                        OrderedValue::Array(groups.iter().map(MatcherGroup::to_claude).collect()),
                    )
                })
                .collect(),
        )
    }

    /// The `[[hooks.<Event>]]` text of a `hook.toml` holding just these hooks.
    pub fn to_toml(&self) -> String {
        let mut root = toml::map::Map::new();
        root.insert("hooks".into(), to_toml_value(&self.to_claude_json()));
        toml::to_string(&toml::Value::Table(root)).expect("hooks are TOML-representable")
    }

    pub fn events(&self) -> impl Iterator<Item = (&str, &[MatcherGroup])> {
        self.events
            .iter()
            .map(|(event, groups)| (event.as_str(), groups.as_slice()))
    }

    pub fn declares(&self, event: &str) -> bool {
        self.events.contains_key(event)
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(super) fn remove_event(&mut self, event: &str) {
        self.events.remove(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_JSON: &str = r#"{"hooks":{
        "PreToolUse":[{"matcher":"Bash","hooks":[
            {"type":"command","command":"bash run.sh","timeout":10,"if":"Bash(*tmux*)",
             "onFailure":"block","statusMessage":"checking","async":false,
             "commandWindows":"run.cmd","shell":"bash"},
            {"type":"http","url":"https://example.test/hook","headers":{"X-A":"b"}}],
          "note":"kept"}],
        "SessionStart":[{"hooks":[{"type":"command","command":"echo hi","timeout":1.5}]}]}}"#;

    #[test]
    fn claude_json_round_trips_through_toml() {
        let (hooks, dropped) = PortableHooks::from_claude_json(CLAUDE_JSON).unwrap();
        assert!(dropped.is_empty());
        let toml_text = hooks.to_toml();
        assert!(toml_text.contains("[[hooks.PreToolUse]]"), "{toml_text}");
        assert!(
            toml_text.contains("[[hooks.PreToolUse.hooks]]"),
            "{toml_text}"
        );
        let value: toml::Value = toml::from_str(&toml_text).unwrap();
        let again = PortableHooks::from_toml(value.get("hooks").unwrap()).unwrap();
        assert_eq!(again, hooks);
        // Both ways land on the JSON we started from.
        let original: OrderedValue = serde_json::from_str(CLAUDE_JSON).unwrap();
        assert_eq!(
            again.to_claude_json().to_json(),
            unwrap_hooks_object(original).to_json()
        );
    }

    #[test]
    fn unknown_fields_are_kept_and_known_ones_typed() {
        let (hooks, _) = PortableHooks::from_claude_json(CLAUDE_JSON).unwrap();
        let (_, groups) = hooks.events().next().unwrap();
        assert_eq!(groups[0].extra["note"], OrderedValue::String("kept".into()));
        let command = &groups[0].handlers[0];
        assert_eq!(command.on_failure.as_deref(), Some("block"));
        assert_eq!(
            command.extra["commandWindows"],
            OrderedValue::String("run.cmd".into())
        );
        assert!(!command.extra.contains_key("timeout"));
        assert!(groups[0].handlers[1].extra.contains_key("url"));
    }

    #[test]
    fn json_null_is_dropped_and_reported() {
        let (hooks, dropped) = PortableHooks::from_claude_json(
            r#"{"Stop":[{"hooks":[{"type":"mcp_tool","server":"s","tool":"t",
                "input":{"a":null,"b":[1,null]},"statusMessage":null}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            dropped,
            [
                "hooks.Stop[0].hooks[0].input.a",
                "hooks.Stop[0].hooks[0].input.b[1]",
                "hooks.Stop[0].hooks[0].statusMessage",
            ]
        );
        assert!(!hooks.to_toml().contains("null"));
    }

    #[test]
    fn mistyped_known_fields_are_refused() {
        for (json, needle) in [
            (
                r#"{"Stop":[{"hooks":[{"type":"command","timeout":"5"}]}]}"#,
                "`timeout` must be a number",
            ),
            (
                r#"{"Stop":[{"hooks":[{"command":"x"}]}]}"#,
                "`type` is required",
            ),
            (
                r#"{"Stop":[{"matcher":1,"hooks":[]}]}"#,
                "`matcher` must be a string",
            ),
            (r#"{"Stop":[{"matcher":"x"}]}"#, "`hooks` is required"),
            (r#"{"Stop":{}}"#, "must be an array of tables"),
        ] {
            let error = PortableHooks::from_claude_json(json).unwrap_err();
            assert!(error.contains(needle), "{json}: {error}");
        }
    }
}
