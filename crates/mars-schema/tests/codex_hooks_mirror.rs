//! Every Codex document the renderer writes must load in Codex 0.164.0.
//!
//! Codex drops its whole `hooks.json` (the user's hooks too) when one handler fails to
//! deserialize, so the renderer's hand-kept list of what it passes is checked here against
//! mirrored copies of Codex's own config structs and skip rules. The mirror is the oracle:
//! when Codex changes, update it from the cited source and this test says what the renderer
//! must now drop. Source: openai/codex `rust-v0.164.0`.

use std::collections::BTreeMap;

use mars_schema::hooks::Tokens;
use mars_schema::hooks::portable::PortableHooks;
use serde::Deserialize;

// codex-rs/config/src/hook_config.rs L11-17
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HooksFile {
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    hooks: HookEventsToml,
}

// L39-69: no `deny_unknown_fields`, so a Claude-only event inside `hooks` is ignored.
#[derive(Debug, Default, Deserialize)]
struct HookEventsToml {
    #[serde(rename = "PreModelRequest", default)]
    pre_model_request: Vec<MatcherGroup>,
    #[serde(rename = "PostModelRequest", default)]
    post_model_request: Vec<MatcherGroup>,
    #[serde(rename = "PreToolUse", default)]
    pre_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "PermissionRequest", default)]
    permission_request: Vec<MatcherGroup>,
    #[serde(rename = "PostToolUse", default)]
    post_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "PreCompact", default)]
    pre_compact: Vec<MatcherGroup>,
    #[serde(rename = "PostCompact", default)]
    post_compact: Vec<MatcherGroup>,
    #[serde(rename = "SessionStart", default)]
    session_start: Vec<MatcherGroup>,
    #[serde(rename = "SessionEnd", default)]
    session_end: Vec<MatcherGroup>,
    #[serde(rename = "UserPromptSubmit", default)]
    user_prompt_submit: Vec<MatcherGroup>,
    #[serde(rename = "SubagentStart", default)]
    subagent_start: Vec<MatcherGroup>,
    #[serde(rename = "SubagentStop", default)]
    subagent_stop: Vec<MatcherGroup>,
    #[serde(rename = "Stop", default)]
    stop: Vec<MatcherGroup>,
    #[serde(rename = "Interrupt", default)]
    interrupt: Vec<MatcherGroup>,
}

// L168-177: no `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
struct MatcherGroup {
    #[serde(default)]
    #[allow(dead_code)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<HookHandlerConfig>,
}

// L180-221: an internally tagged enum; a field of the wrong type fails the whole file.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum HookHandlerConfig {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default, rename = "commandWindows", alias = "command_windows")]
        command_windows: Option<String>,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default)]
        r#async: bool,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
        #[serde(default, rename = "additionalContextLimit")]
        additional_context_limit: Option<usize>,
    },
    #[serde(rename = "mcp_tool")]
    McpTool {
        server: String,
        tool: String,
        #[serde(default, deserialize_with = "mcp_tool_input")]
        input: serde_json::Map<String, serde_json::Value>,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
    },
    #[serde(rename = "prompt")]
    Prompt {},
    #[serde(rename = "agent")]
    Agent {},
}

// L221-235: the input must be representable as TOML (so no `null`).
fn mcp_tool_input<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<serde_json::Map<String, serde_json::Value>, D::Error> {
    let input = serde_json::Map::deserialize(deserializer)?;
    toml::Value::try_from(&input).map_err(serde::de::Error::custom)?;
    Ok(input)
}

impl HookEventsToml {
    fn groups(&self) -> [(&'static str, &Vec<MatcherGroup>); 14] {
        [
            ("PreModelRequest", &self.pre_model_request),
            ("PostModelRequest", &self.post_model_request),
            ("PreToolUse", &self.pre_tool_use),
            ("PermissionRequest", &self.permission_request),
            ("PostToolUse", &self.post_tool_use),
            ("PreCompact", &self.pre_compact),
            ("PostCompact", &self.post_compact),
            ("SessionStart", &self.session_start),
            ("SessionEnd", &self.session_end),
            ("UserPromptSubmit", &self.user_prompt_submit),
            ("SubagentStart", &self.subagent_start),
            ("SubagentStop", &self.subagent_stop),
            ("Stop", &self.stop),
            ("Interrupt", &self.interrupt),
        ]
    }
}

/// The handlers Codex runs and the ones its loader skips, from the skip rules in
/// codex-rs/hooks/src/engine/discovery.rs L548-695: an empty command, an `mcp_tool` on
/// `SessionEnd` or with an empty server or tool, and `prompt` and `agent` handlers.
fn loaded(events: &HookEventsToml) -> (usize, Vec<String>) {
    let (mut run, mut skipped) = (0, Vec::new());
    for (event, groups) in events.groups() {
        for handler in groups.iter().flat_map(|group| &group.hooks) {
            let problem = match handler {
                HookHandlerConfig::Command { command, .. } if command.trim().is_empty() => {
                    Some("empty command")
                }
                HookHandlerConfig::McpTool { .. } if event == "SessionEnd" => {
                    Some("mcp_tool on SessionEnd")
                }
                HookHandlerConfig::McpTool { server, tool, .. }
                    if server.trim().is_empty() || tool.trim().is_empty() =>
                {
                    Some("empty server or tool")
                }
                HookHandlerConfig::Prompt {} | HookHandlerConfig::Agent {} => {
                    Some("prompt or agent handler")
                }
                _ => None,
            };
            match problem {
                Some(problem) => skipped.push(format!("{event}: {problem}")),
                None => run += 1,
            }
        }
    }
    (run, skipped)
}

fn parse(document: &serde_json::Value) -> Result<HookEventsToml, serde_json::Error> {
    serde_json::from_value::<HooksFile>(document.clone()).map(|file| file.hooks)
}

/// The document mars writes for Codex: the renderer's events under `hooks`.
fn codex_document(claude_json: &str) -> (serde_json::Value, PortableHooks) {
    let (hooks, _) = PortableHooks::from_claude_json(claude_json).unwrap();
    let rendered = hooks.render(
        &mars_schema::hooks::targets::target(".codex")
            .unwrap()
            .profile,
        &Tokens {
            hook_dir: "/p/.codex/hooks/audit",
            hook_target: ".codex",
        },
    );
    let events: BTreeMap<_, Vec<_>> = rendered
        .events()
        .iter()
        .map(|(event, entries)| (event, entries.iter().map(|e| e.to_json()).collect()))
        .collect();
    (serde_json::json!({ "hooks": events }), hooks)
}

/// Every Claude event, and every handler shape a Claude author can write.
const EVERYTHING: &str = r#"{
  "SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"echo a","timeout":1.2}]}],
  "UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo b","statusMessage":"s","async":true}]}],
  "PreToolUse":[{"matcher":"Bash|Edit","hooks":[
    {"type":"command","command":"echo c","timeout":0.5,"if":"Bash(*)","onFailure":"block",
     "commandWindows":"c.cmd","additionalContextLimit":10,"once":true,"shell":"bash"},
    {"type":"http","url":"https://example.test","timeout":3},
    {"type":"prompt","prompt":"p"},
    {"type":"agent"},
    {"type":"mcp_tool","server":"s","tool":"t","input":{"a":1,"b":[1,"x"],"c":{"d":true}},"timeout":2.5},
    {"type":"command"},
    {"type":"command","command":"  "},
    {"type":"command","command":"x","timeout":-3},
    {"type":"command","command":"x","commandWindows":4},
    {"type":"mcp_tool","server":"","tool":"t"},
    {"type":"mcp_tool","server":"s","tool":"t","input":[]}],
   "claudeOnlyGroupField":1}],
  "PermissionRequest":[{"hooks":[{"type":"command","command":"echo d"}]}],
  "PostToolUse":[{"hooks":[{"type":"mcp_tool","server":"s","tool":"t"}]}],
  "PostToolUseFailure":[{"hooks":[{"type":"command","command":"echo e"}]}],
  "SubagentStart":[{"hooks":[{"type":"command","command":"echo f"}]}],
  "SubagentStop":[{"hooks":[{"type":"command","command":"echo g"}]}],
  "Stop":[{"hooks":[{"type":"command","command":"echo h"}]}],
  "PreCompact":[{"hooks":[{"type":"command","command":"echo i"}]}],
  "PostCompact":[{"hooks":[{"type":"command","command":"echo j"}]}],
  "SessionEnd":[{"hooks":[{"type":"command","command":"echo k"},
                          {"type":"mcp_tool","server":"s","tool":"t"}]}],
  "Notification":[{"hooks":[{"type":"command","command":"echo n"}]}],
  "TaskCreated":[{"hooks":[{"type":"command","command":"echo t"}]}]
}"#;

// codex-rs/hooks/src/engine/matcher.rs `HookMatcher::new`: what discovery compiles.
fn codex_admits_matcher(pattern: &str) -> bool {
    pattern.is_empty()
        || pattern == "*"
        || pattern
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '|')
        || regex::Regex::new(pattern).is_ok()
}

#[test]
fn every_rendered_matcher_compiles_in_codex() {
    // Patterns that work on Claude (JavaScript regex) and fail in Rust's `regex`.
    let (document, _) = codex_document(
        r#"{"PreToolUse":[
            {"matcher":"^(?!Read).*","hooks":[{"type":"command","command":"a"}]},
            {"matcher":"(a)\\1","hooks":[{"type":"command","command":"b"}]},
            {"matcher":"Bash","hooks":[{"type":"command","command":"c"}]},
            {"matcher":"^mcp__(github|slack)__.*$","hooks":[{"type":"command","command":"d"}]}]}"#,
    );
    let groups = document["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{document:#}");
    for group in groups {
        let matcher = group["matcher"].as_str().unwrap();
        assert!(codex_admits_matcher(matcher), "Codex rejects `{matcher}`");
    }
    assert!(!codex_admits_matcher("^(?!Read).*"));
}

#[test]
fn every_rendered_codex_document_loads_with_no_handler_skipped() {
    let (document, _) = codex_document(EVERYTHING);
    let events = parse(&document).expect("Codex must deserialize what mars writes");
    let (run, skipped) = loaded(&events);
    assert!(skipped.is_empty(), "Codex would skip: {skipped:?}");
    // PreToolUse keeps its command and its mcp_tool; each of the other ten Codex events
    // keeps one handler (SessionEnd loses its mcp_tool).
    assert_eq!(run, 12, "{document:#}");
    let pre = &events.pre_tool_use;
    assert_eq!(pre.len(), 1);
    assert_eq!(pre[0].hooks.len(), 2);
}

#[test]
fn the_mirror_rejects_what_the_renderer_leaves_out() {
    // Without the renderer's checks the same input fails or is skipped, so the test above
    // can fail: Claude's JSON passed through as is.
    let (hooks, _) = PortableHooks::from_claude_json(EVERYTHING).unwrap();
    let raw = hooks.to_claude_json().to_json();
    let events = parse(&serde_json::json!({ "hooks": raw }));
    assert!(events.is_err(), "a float timeout must fail Codex's parse");

    let skipped = parse(&serde_json::json!({"hooks": {"SessionEnd": [
        {"hooks": [{"type": "mcp_tool", "server": "s", "tool": "t"},
                   {"type": "prompt"}, {"type": "command", "command": " "}]}]}}))
    .unwrap();
    assert_eq!(loaded(&skipped).1.len(), 3);
    assert!(
        parse(&serde_json::json!({"hooks": {"Stop": [{"hooks": [
        {"type": "mcp_tool", "server": "s", "tool": "t", "input": {"a": null}}]}]}}))
        .is_err()
    );
    // A Claude-only event beside `hooks` fails the whole file.
    assert!(parse(&serde_json::json!({"hooks": {}, "Notification": []})).is_err());
}

#[test]
fn each_codex_event_is_a_field_of_the_mirror() {
    let (document, _) = codex_document(EVERYTHING);
    let events = parse(&document).unwrap();
    let named: Vec<_> = events
        .groups()
        .iter()
        .filter(|(_, groups)| !groups.is_empty())
        .map(|(event, _)| *event)
        .collect();
    let rendered: Vec<_> = document["hooks"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    // Anything the renderer wrote that the mirror does not name would be ignored by Codex.
    for event in &rendered {
        assert!(named.contains(event), "Codex ignores `{event}`");
    }
}

#[test]
fn the_target_table_names_every_event_codex_reads() {
    // `trust_positions` counts only these: a key Codex does not read needs no review.
    let target = mars_schema::hooks::targets::target(".codex").unwrap();
    let mirror = HookEventsToml::default();
    for (event, _) in mirror.groups() {
        assert!(
            target.reads(event),
            "the table does not know Codex's `{event}`"
        );
    }
    let read = mirror.groups().len();
    let known = target.profile.events.len() + target.other_events.len();
    assert_eq!(known, read, "the table names an event Codex does not read");
    assert!(!target.reads("Notification"));
}
