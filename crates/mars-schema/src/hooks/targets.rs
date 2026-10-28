//! What each target root takes of a hook (D11): one [`Target`] per root, holding where its
//! hooks are placed, the events it accepts, the harness version that table was last checked
//! against, how `[hooks]` is rendered for it, and what its generated files are called.
//!
//! The binary adds only where a merge target's file lives on disk. Nothing else about a
//! target is kept anywhere else.

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
    /// The hook's events are merged into the target's hook config file.
    Merge,
    /// The hook is installed as whole files in `dir`, under the target root.
    File { dir: &'static str },
}

/// How `[hooks]` becomes a target's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rules {
    /// Claude takes the table as written.
    Claude,
    /// Codex's parser: handlers and matchers it cannot run are left out (D17).
    Codex,
    /// Generated TypeScript for `harness`; not rendered yet.
    TypeScript { harness: &'static str },
    /// Nothing is rendered for the target, as a whole; `reason` says why.
    Withheld { reason: &'static str },
}

/// What a renderer needs of a target: the events it accepts and the rules its parser applies.
/// The two vary independently: the plugin tree's `hooks.json` is Claude's events under
/// Codex's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// Every event the target accepts.
    pub events: &'static [Event],
    pub rules: Rules,
}

impl Profile {
    /// Whether the rules render `[hooks]` at all.
    pub fn renders(&self) -> bool {
        matches!(self.rules, Rules::Claude | Rules::Codex)
    }
}

/// One target root's hook rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub root: &'static str,
    /// The harness version `profile.events` was last checked against; `None` while the
    /// target's rows are unchecked.
    pub verified_against: Option<&'static str>,
    pub placement: Placement,
    pub profile: Profile,
}

/// Where a file-placement target's generated file gets its suffix: the hook name ends in
/// neither, so a generated file never collides with an authored one.
const GENERATED_SUFFIX: &str = super::RESERVED_NAME_SUFFIX;

impl Target {
    /// Whether the target installs whole files rather than merging entries.
    pub fn is_file(&self) -> bool {
        matches!(self.placement, Placement::File { .. })
    }

    /// The path, relative to the target root, of the hook's own fragment file; `None` for a
    /// merge target, which has no per-hook file.
    pub fn fragment_path(&self, hook_name: &str) -> Option<String> {
        let Placement::File { dir } = self.placement else {
            return None;
        };
        Some(format!("{dir}/mars-{hook_name}.ts"))
    }

    /// The path, relative to the target root, of the file generated from the hook's
    /// `[hooks]`; `None` for a merge target.
    pub fn generated_path(&self, hook_name: &str) -> Option<String> {
        let Placement::File { dir } = self.placement else {
            return None;
        };
        Some(format!("{dir}/mars-{hook_name}{GENERATED_SUFFIX}.ts"))
    }
}

// Claude Code 2.1.312 (`CLAUDE.verified_against`); event names from https://code.claude.com/docs/en/hooks (2026-07-24),
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

const TARGETS: &[Target] = &[
    Target {
        root: ".claude",
        verified_against: Some("Claude Code 2.1.312"),
        placement: Placement::Merge,
        profile: Profile {
            events: CLAUDE_EVENTS,
            rules: Rules::Claude,
        },
    },
    Target {
        root: ".codex",
        verified_against: Some("codex-cli 0.164.0"),
        placement: Placement::Merge,
        profile: Profile {
            events: CODEX_EVENTS,
            rules: Rules::Codex,
        },
    },
    Target {
        root: ".cursor",
        verified_against: Some("cursor-agent 2026.07.16"),
        placement: Placement::Merge,
        profile: Profile {
            events: CURSOR_EVENTS,
            rules: Rules::Withheld {
                reason: "mars renders no generated hooks for Cursor before 0.26",
            },
        },
    },
    // The events a generated adapter handles are listed when its renderer lands.
    Target {
        root: ".opencode",
        verified_against: None,
        placement: Placement::File { dir: "plugins" },
        profile: Profile {
            events: &[],
            rules: Rules::TypeScript {
                harness: "OpenCode",
            },
        },
    },
    Target {
        root: ".pi",
        verified_against: None,
        placement: Placement::File { dir: "extensions" },
        profile: Profile {
            events: &[],
            rules: Rules::TypeScript { harness: "Pi" },
        },
    },
];

/// The hook rules of `root`; `None` when it has no hook mechanism.
pub fn target(root: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|target| target.root == root)
}

/// Every target that takes hooks.
pub fn all() -> &'static [Target] {
    TARGETS
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
