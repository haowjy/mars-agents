//! A portable hook rendered for one target: finished entries per event, and what was left
//! out. Renderers emit only what the target's parser accepts (D17); they never pass Claude's
//! JSON through blindly.

use std::collections::BTreeMap;

use serde_json::Number;

use super::Tokens;
use super::portable::{Handler, MatcherGroup, PortableHooks};
use super::targets::{self, Profile, Rules};
use crate::ordered::OrderedValue;

/// Something a target does not run. `event` is `None` when the whole target is skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub event: Option<String>,
    pub reason: String,
}

/// One profile's rendering of a hook's `[hooks]` table.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub output: Output,
    pub skips: Vec<Skip>,
}

/// What a rendering yields, in the shape the target's placement takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// Finished entries per native event, for a hook config file. An event with nothing left
    /// is absent.
    Merge {
        events: BTreeMap<String, Vec<OrderedValue>>,
        /// Events the target does not list that the hook's `unchecked = true` passed through.
        unchecked_events: Vec<String>,
    },
    /// The text of the one file generated from the table; `None` when nothing is generated.
    File { text: Option<String> },
}

impl Rendered {
    fn skip(&mut self, event: &str, reason: String) {
        self.skips.push(Skip {
            event: Some(event.to_owned()),
            reason,
        });
    }

    /// Whether nothing was rendered.
    pub fn is_empty(&self) -> bool {
        match &self.output {
            Output::Merge { events, .. } => events.is_empty(),
            Output::File { text } => text.is_none(),
        }
    }

    /// The merge shape's events; none for a file.
    pub fn events(&self) -> &BTreeMap<String, Vec<OrderedValue>> {
        static NONE: BTreeMap<String, Vec<OrderedValue>> = BTreeMap::new();
        match &self.output {
            Output::Merge { events, .. } => events,
            Output::File { .. } => &NONE,
        }
    }

    /// The merge shape's unchecked events; none for a file.
    pub fn unchecked_events(&self) -> &[String] {
        match &self.output {
            Output::Merge {
                unchecked_events, ..
            } => unchecked_events,
            Output::File { .. } => &[],
        }
    }
}

impl PortableHooks {
    /// This table as `profile` takes it, with `tokens` substituted in every string.
    pub fn render(&self, profile: &Profile, tokens: &Tokens<'_>) -> Rendered {
        let mut rendered = match profile.rules {
            Rules::Claude => self.render_claude(profile),
            Rules::Codex => self.render_codex(profile),
            Rules::Withheld { reason } => Rendered {
                output: Output::Merge {
                    events: BTreeMap::new(),
                    unchecked_events: Vec::new(),
                },
                skips: vec![Skip {
                    event: None,
                    reason: reason.into(),
                }],
            },
            Rules::TypeScript { harness } => {
                let mut rendered = Rendered {
                    output: Output::File { text: None },
                    skips: Vec::new(),
                };
                for (event, _) in self.events() {
                    rendered.skip(
                        event,
                        format!("mars does not render hooks for {harness} yet"),
                    );
                }
                rendered
            }
        };
        if let Output::Merge { events, .. } = &mut rendered.output {
            for entries in events.values_mut() {
                entries
                    .iter_mut()
                    .for_each(|entry| tokens.substitute_value(entry));
            }
        }
        rendered
    }

    /// Whether `render` yields anything under `profile`.
    pub fn renders_on(&self, profile: &Profile) -> bool {
        !self.render(profile, &Tokens::default()).is_empty()
    }

    /// Events on which a handler sets `if`, though Claude never runs such a handler there.
    pub fn conditions_on_non_tool_events(&self) -> Vec<&str> {
        self.events()
            .filter(|(event, groups)| {
                let tool_event = targets::claude_event(event).is_some_and(|e| e.is_tool_event());
                !tool_event
                    && groups
                        .iter()
                        .flat_map(|group| &group.handlers)
                        .any(|handler| handler.condition.is_some())
            })
            .map(|(event, _)| event)
            .collect()
    }

    /// Identity: Claude takes the table as written. An event the profile does not list is one
    /// the author passed with `unchecked = true`.
    fn render_claude(&self, profile: &Profile) -> Rendered {
        let mut events = BTreeMap::new();
        let mut unchecked_events = Vec::new();
        for (event, groups) in self.events() {
            if !profile.events.iter().any(|e| e.name == event) {
                unchecked_events.push(event.to_owned());
            }
            events.insert(
                event.to_owned(),
                groups.iter().map(MatcherGroup::to_claude).collect(),
            );
        }
        Rendered {
            output: Output::Merge {
                events,
                unchecked_events,
            },
            skips: Vec::new(),
        }
    }

    fn render_codex(&self, profile: &Profile) -> Rendered {
        let mut rendered = Rendered {
            output: Output::Merge {
                events: BTreeMap::new(),
                unchecked_events: Vec::new(),
            },
            skips: Vec::new(),
        };
        let mut rendered_events = BTreeMap::new();
        for (event, groups) in self.events() {
            if targets::claude_event(event).is_none() {
                rendered.skip(
                    event,
                    "not an event mars knows, so `unchecked` renders it on Claude only".into(),
                );
                continue;
            }
            if !profile.events.iter().any(|e| e.name == event) {
                rendered.skip(event, "Codex has no such event".into());
                continue;
            }
            let matched = profile
                .events
                .iter()
                .any(|e| e.name == event && e.matcher.is_some());
            let mut entries = Vec::new();
            for (g, group) in groups.iter().enumerate() {
                if let (true, Some(matcher)) = (matched, &group.matcher)
                    && let Err(reason) = codex_matcher(matcher)
                {
                    rendered.skip(
                        event,
                        format!(
                            "matcher group {g}: Codex cannot compile matcher `{matcher}`: {reason}"
                        ),
                    );
                    continue;
                }
                let mut kept = Vec::new();
                for (h, handler) in group.handlers.iter().enumerate() {
                    match codex_handler(event, handler) {
                        Ok(handler) => kept.push(handler),
                        Err(reason) => rendered
                            .skip(event, format!("matcher group {g}, handler {h}: {reason}")),
                    }
                }
                if !kept.is_empty() {
                    entries.push(group.to_value(&kept));
                }
            }
            if !entries.is_empty() {
                rendered_events.insert(event.to_owned(), entries);
            }
        }
        rendered.output = Output::Merge {
            events: rendered_events,
            unchecked_events: Vec::new(),
        };
        rendered
    }
}

/// Whether Codex 0.164.0 admits a group's `matcher`. Codex compiles it at discovery, and a
/// group whose matcher fails is skipped (`HookMatcher::new`, codex-rs/hooks/src/engine/
/// matcher.rs; `append_matcher_groups`, discovery.rs L527-541): empty, `*` and plain
/// `A|B` name lists are exact, anything else is a Rust `regex`, which has no look-around or
/// backreferences although Claude's JavaScript regex does. Codex reads the matcher only on
/// the events that have a matcher field (`matcher_pattern_for_event`, events/common.rs).
fn codex_matcher(pattern: &str) -> Result<(), String> {
    use regex_syntax::Error;
    if pattern.is_empty()
        || pattern == "*"
        || pattern
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '|')
    {
        return Ok(());
    }
    regex_syntax::Parser::new()
        .parse(pattern)
        .map(drop)
        .map_err(|error| match error {
            Error::Parse(error) => error.kind().to_string(),
            Error::Translate(error) => error.kind().to_string(),
            other => other.to_string(),
        })
}

/// A handler as Codex 0.164.0 parses it, or why Codex would not run it. Codex rejects its
/// whole `hooks.json` over one handler it cannot parse, so each field Codex types is checked
/// here: `HookHandlerConfig`, codex-rs/config/src/hook_config.rs L180-221; skip rules in
/// codex-rs/hooks/src/engine/discovery.rs L548-695.
fn codex_handler(event: &str, handler: &Handler) -> Result<Handler, String> {
    let extra = |key: &str| handler.extra.get(key);
    let text = |key: &str| match extra(key) {
        None => Ok(None),
        Some(OrderedValue::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(format!("`{key}` must be a string for Codex")),
    };
    match handler.kind.as_str() {
        "command" => {
            if handler
                .command
                .as_deref()
                .is_none_or(|c| c.trim().is_empty())
            {
                return Err("a command handler needs a non-empty `command`".into());
            }
            text("commandWindows")?;
            match extra("additionalContextLimit") {
                None => {}
                Some(OrderedValue::Number(n)) if n.as_u64().is_some() => {}
                Some(_) => return Err("`additionalContextLimit` must be a whole number".into()),
            }
        }
        "mcp_tool" => {
            if event == "SessionEnd" {
                return Err("Codex runs no `mcp_tool` handler on SessionEnd".into());
            }
            for key in ["server", "tool"] {
                if text(key)?.is_none_or(|value| value.trim().is_empty()) {
                    return Err(format!("an `mcp_tool` handler needs a non-empty `{key}`"));
                }
            }
            if extra("input").is_some_and(|input| !matches!(input, OrderedValue::Object(_))) {
                return Err("`input` must be a table".into());
            }
        }
        "prompt" | "agent" => {
            return Err(format!(
                "Codex parses `{}` handlers but never runs them",
                handler.kind
            ));
        }
        other => {
            return Err(format!(
                "Codex runs `command` and `mcp_tool` handlers, not `{other}`"
            ));
        }
    }
    let mut handler = handler.clone();
    handler.timeout = handler.timeout.as_ref().map(whole_seconds).transpose()?;
    Ok(handler)
}

/// Codex reads `timeout` as an unsigned integer: a fractional one rounds up.
fn whole_seconds(timeout: &Number) -> Result<Number, String> {
    if let Some(seconds) = timeout.as_u64() {
        return Ok(seconds.into());
    }
    match timeout.as_f64() {
        Some(seconds) if seconds >= 0.0 && seconds <= u64::MAX as f64 => {
            Ok((seconds.ceil() as u64).into())
        }
        _ => Err("`timeout` must be a non-negative number of seconds".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(root: &str) -> Profile {
        targets::target(root).unwrap().profile
    }

    fn hooks(json: &str) -> PortableHooks {
        PortableHooks::from_claude_json(json).unwrap().0
    }

    fn tokens() -> Tokens<'static> {
        Tokens {
            hook_dir: "/p/.codex/hooks/audit",
            hook_target: ".codex",
        }
    }

    const MIXED: &str = r#"{
        "PreToolUse":[{"matcher":"Bash","hooks":[
            {"type":"command","command":"bash ${MARS_HOOK_DIR}/run.sh ${MARS_HOOK_TARGET}",
             "timeout":2.5,"if":"Bash(*tmux*)","onFailure":"block","statusMessage":"m"},
            {"type":"http","url":"https://example.test"},
            {"type":"prompt"}]}],
        "SessionEnd":[{"hooks":[{"type":"mcp_tool","server":"s","tool":"t"},
                                {"type":"command","command":"echo bye"}]}],
        "Notification":[{"hooks":[{"type":"command","command":"echo n"}]}]}"#;

    #[test]
    fn claude_gets_the_table_as_written_with_tokens_substituted() {
        let rendered = hooks(MIXED).render(&profile(".claude"), &tokens());
        assert!(rendered.skips.is_empty());
        let json = serde_json::to_string(&rendered.events()["PreToolUse"][0].to_json()).unwrap();
        assert!(
            json.contains("bash /p/.codex/hooks/audit/run.sh .codex"),
            "{json}"
        );
        assert!(json.contains(r#""timeout":2.5"#));
        assert_eq!(rendered.events().len(), 3);
    }

    #[test]
    fn codex_gets_what_codex_parses_and_reports_the_rest() {
        let rendered = hooks(MIXED).render(&profile(".codex"), &tokens());
        let pre = serde_json::to_string(
            &rendered.events()["PreToolUse"]
                .iter()
                .map(OrderedValue::to_json)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        // The fractional timeout rounds up; `if` and `onFailure` stay (Codex ignores them).
        assert_eq!(
            pre,
            r#"[{"hooks":[{"command":"bash /p/.codex/hooks/audit/run.sh .codex","if":"Bash(*tmux*)","onFailure":"block","statusMessage":"m","timeout":3,"type":"command"}],"matcher":"Bash"}]"#
        );
        // SessionEnd keeps the command and drops the mcp_tool; Notification has no Codex event.
        let end = serde_json::to_string(&rendered.events()["SessionEnd"][0].to_json()).unwrap();
        assert_eq!(
            end,
            r#"{"hooks":[{"command":"echo bye","type":"command"}]}"#
        );
        assert!(!rendered.events().contains_key("Notification"));
        let reasons: Vec<_> = rendered
            .skips
            .iter()
            .map(|s| format!("{}: {}", s.event.as_deref().unwrap(), s.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                "Notification: Codex has no such event",
                "PreToolUse: matcher group 0, handler 1: Codex runs `command` and `mcp_tool` handlers, not `http`",
                "PreToolUse: matcher group 0, handler 2: Codex parses `prompt` handlers but never runs them",
                "SessionEnd: matcher group 0, handler 0: Codex runs no `mcp_tool` handler on SessionEnd",
            ]
        );
    }

    #[test]
    fn codex_skips_a_group_whose_matcher_it_cannot_compile() {
        let table = hooks(
            r#"{"PreToolUse":[
                {"matcher":"^(?!Read).*","hooks":[{"type":"command","command":"a"}]},
                {"matcher":"Bash|Edit","hooks":[{"type":"command","command":"b"}]},
                {"matcher":"mcp__.*","hooks":[{"type":"command","command":"c"}]},
                {"matcher":"*","hooks":[{"type":"command","command":"d"}]}],
               "Stop":[{"matcher":"(?<=x)y","hooks":[{"type":"command","command":"e"}]}]}"#,
        );
        let rendered = table.render(&profile(".codex"), &tokens());
        let commands: Vec<_> = rendered.events()["PreToolUse"]
            .iter()
            .map(|g| {
                g.to_json()["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(commands, ["b", "c", "d"]);
        // Codex reads no matcher on Stop, so that group stays.
        assert!(rendered.events().contains_key("Stop"));
        assert_eq!(rendered.skips.len(), 1);
        assert!(
            rendered.skips[0].reason.contains("matcher group 0")
                && rendered.skips[0].reason.contains("look-around"),
            "{:?}",
            rendered.skips
        );
        // Claude takes all of it.
        assert_eq!(
            table.render(&profile(".claude"), &tokens()).events()["PreToolUse"].len(),
            4
        );
    }

    #[test]
    fn a_group_with_no_handler_left_is_left_out() {
        let rendered = hooks(r#"{"Stop":[{"hooks":[{"type":"http","url":"u"}]}]}"#)
            .render(&profile(".codex"), &tokens());
        assert!(rendered.is_empty());
        assert_eq!(rendered.skips.len(), 1);
    }

    #[test]
    fn opencode_pi_and_cursor_render_nothing_and_say_why() {
        let table = hooks(MIXED);
        for root in [".opencode", ".pi"] {
            let rendered = table.render(&profile(root), &tokens());
            assert!(rendered.is_empty());
            assert_eq!(rendered.skips.len(), 3, "{root}");
            assert!(rendered.skips[0].reason.contains("yet"));
        }
        let cursor = table.render(&profile(".cursor"), &tokens());
        assert!(cursor.is_empty());
        assert_eq!(cursor.skips.len(), 1);
        assert_eq!(cursor.skips[0].event, None);
    }

    #[test]
    fn if_on_a_non_tool_event_is_named() {
        let table = hooks(
            r#"{"PreToolUse":[{"hooks":[{"type":"command","command":"x","if":"Bash(*)"}]}],
                "Stop":[{"hooks":[{"type":"command","command":"x","if":"Bash(*)"}]}]}"#,
        );
        assert_eq!(table.conditions_on_non_tool_events(), ["Stop"]);
    }
}
