//! What each target root takes of a hook: the table of events (D11), where it keeps them,
//! and the version of the harness the table was last checked against.
//!
//! One table per target. The renderers (`render`) read their event lists from here, and the
//! binary reads `placement` to know whether a target merges entries or installs a file.

/// A hook event of one target and the input field its `matcher` tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub name: &'static str,
    /// The input field the matcher tests (`tool_name`, `source`, …). `None` means no field is
    /// recorded for this event; only the events a generated adapter renders need one.
    pub matcher: Option<&'static str>,
}

impl Event {
    /// A tool event: its handlers can carry Claude's `if` filter.
    pub fn is_tool_event(&self) -> bool {
        self.matcher == Some("tool_name")
    }
}

const fn event(name: &'static str) -> Event {
    Event {
        name,
        matcher: None,
    }
}

const fn matched(name: &'static str, field: &'static str) -> Event {
    Event {
        name,
        matcher: Some(field),
    }
}

/// How a target root takes a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The hook's events are merged into the target's hook config file; `events` is every
    /// event the target's parser accepts.
    Merge { events: &'static [Event] },
    /// The hook is installed as opaque files.
    File,
}

// Claude Code 2.1.312; event names from https://code.claude.com/docs/en/hooks (2026-07-24),
// matcher fields from research/claude-codex-cursor-answers.md § Matchers per moment.
const CLAUDE_EVENTS: &[Event] = &[
    matched("SessionStart", "source"),
    event("Setup"),
    event("UserPromptSubmit"),
    event("UserPromptExpansion"),
    matched("PreToolUse", "tool_name"),
    matched("PermissionRequest", "tool_name"),
    matched("PermissionDenied", "tool_name"),
    matched("PostToolUse", "tool_name"),
    matched("PostToolUseFailure", "tool_name"),
    event("PostToolBatch"),
    matched("SubagentStart", "agent_type"),
    matched("SubagentStop", "agent_type"),
    event("TaskCreated"),
    event("TaskCompleted"),
    event("Stop"),
    event("StopFailure"),
    event("TeammateIdle"),
    matched("PreCompact", "trigger"),
    matched("PostCompact", "trigger"),
    event("Elicitation"),
    event("ElicitationResult"),
    matched("Notification", "notification_type"),
    event("ConfigChange"),
    event("InstructionsLoaded"),
    event("CwdChanged"),
    event("FileChanged"),
    event("WorktreeCreate"),
    event("WorktreeRemove"),
    matched("SessionEnd", "reason"),
];

// codex-cli 0.164.0: `HookEventsToml` (codex-rs/config/src/hook_config.rs L39-69) names
// fourteen events. These are the ones Claude also has; the Codex-only ones (`PreModelRequest`,
// `PostModelRequest`, `Interrupt`) stay in a Codex fragment. `SessionEnd` fires on 0.164.0
// (`reason` is always `other`; research/live-unverified-rows.md); 0.144.4 did not.
const CODEX_EVENTS: &[Event] = &[
    matched("SessionStart", "source"),
    event("UserPromptSubmit"),
    matched("PreToolUse", "tool_name"),
    matched("PermissionRequest", "tool_name"),
    matched("PostToolUse", "tool_name"),
    matched("PreCompact", "trigger"),
    matched("PostCompact", "trigger"),
    matched("SubagentStart", "agent_type"),
    matched("SubagentStop", "agent_type"),
    event("Stop"),
    matched("SessionEnd", "reason"),
];

// cursor-agent 2026.07.16 installed-binary validator, extracted and
// verified by p5714 on 2026-07-24; p5695 provided the docs baseline. Matcher fields are
// recorded when Cursor renders generated hooks (0.26).
const CURSOR_EVENTS: &[Event] = &[
    event("beforeShellExecution"),
    event("beforeMCPExecution"),
    event("afterShellExecution"),
    event("afterMCPExecution"),
    event("beforeReadFile"),
    event("afterFileEdit"),
    event("beforeTabFileRead"),
    event("afterTabFileEdit"),
    event("stop"),
    event("beforeSubmitPrompt"),
    event("afterAgentResponse"),
    event("afterAgentThought"),
    event("sessionStart"),
    event("sessionEnd"),
    event("preCompact"),
    event("subagentStart"),
    event("subagentStop"),
    event("preToolUse"),
    event("postToolUse"),
    event("postToolUseFailure"),
    event("workspaceOpen"),
];

/// How `target_root` takes hooks; `None` when it has no hook mechanism.
pub fn placement(target_root: &str) -> Option<Placement> {
    match target_root {
        ".claude" => Some(Placement::Merge {
            events: CLAUDE_EVENTS,
        }),
        ".codex" => Some(Placement::Merge {
            events: CODEX_EVENTS,
        }),
        ".cursor" => Some(Placement::Merge {
            events: CURSOR_EVENTS,
        }),
        ".opencode" | ".pi" => Some(Placement::File),
        _ => None,
    }
}

/// The targets that render a `[hooks]` table today. OpenCode and Pi join when their
/// generated TypeScript lands; Cursor in 0.26.
pub const PORTABLE_TARGETS: &[&str] = &[".claude", ".codex"];

/// The harness version the target's table was last verified against.
pub fn verified_against(target_root: &str) -> Option<&'static str> {
    match target_root {
        ".claude" => Some("Claude Code 2.1.312"),
        ".codex" => Some("codex-cli 0.164.0"),
        ".cursor" => Some("cursor-agent 2026.07.16"),
        _ => None,
    }
}

/// The events `target_root` merges; empty for a target that does not.
pub fn events(target_root: &str) -> &'static [Event] {
    match placement(target_root) {
        Some(Placement::Merge { events }) => events,
        _ => &[],
    }
}

/// Claude's event of that name: the set a `[hooks]` table may declare.
pub fn claude_event(name: &str) -> Option<&'static Event> {
    CLAUDE_EVENTS.iter().find(|event| event.name == name)
}

/// Claude's event names, for a message.
pub fn claude_event_names() -> String {
    CLAUDE_EVENTS
        .iter()
        .map(|event| event.name)
        .collect::<Vec<_>>()
        .join(", ")
}
