//! What each target root takes of a hook (D11): one [`Target`] row per root, holding where its
//! hooks are placed (the merge file and its trust gate, or the file directory), the events it
//! accepts, the harness version that table was last checked against, how `[hooks]` is
//! rendered for it (for a TypeScript harness, its glue, limits and fragment contract), and
//! what its generated files are called. A new target is one row here; the binary only places
//! what the row describes. Nothing about a target is kept anywhere else.
use super::typescript::{self, Harness};

/// A hook event of one target and the input field its `matcher` tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub name: &'static str,
    /// The input field the matcher tests (`tool_name`, `source`, …). `None` means no field is
    /// recorded for this event; only the events a generated adapter renders need one.
    pub matcher: Option<&'static str>,
    /// Claude Code's default `timeout` for a `command` handler on this event, in
    /// milliseconds. A generated TypeScript hook applies the Claude row's value so it times
    /// out as it would on Claude; the other targets' rows carry the common default unread.
    pub default_timeout_ms: u32,
}

/// The default of every event but the two below. Claude Code docs, "Hook handler fields",
/// `timeout`: https://code.claude.com/docs/en/hooks (read 2026-10-28).
const DEFAULT_TIMEOUT_MS: u32 = 600_000;

impl Event {
    /// The row with a shorter Claude default. The docs lower it to 30 s on
    /// `UserPromptSubmit` (and on the `PreModelSwitch`/`PostModelSwitch`, 10 s on
    /// `MessageDisplay`, which the Claude table below does not list) and give `SessionEnd`
    /// a 1.5 s budget.
    const fn timeout_ms(self, default_timeout_ms: u32) -> Self {
        Self {
            default_timeout_ms,
            ..self
        }
    }

    /// A tool event: its handlers can carry Claude's `if` filter.
    pub fn is_tool_event(&self) -> bool {
        self.matcher == Some("tool_name")
    }
}

const fn event(name: &'static str) -> Event {
    Event {
        name,
        matcher: None,
        default_timeout_ms: DEFAULT_TIMEOUT_MS,
    }
}

const fn matched(name: &'static str, field: &'static str) -> Event {
    Event {
        name,
        matcher: Some(field),
        default_timeout_ms: DEFAULT_TIMEOUT_MS,
    }
}

/// The hook config file of a merge target, where its entries are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeFile {
    /// Under the target root.
    pub file: &'static str,
    /// Keys set at the top of the file before appending (Cursor's `version`).
    pub top_level: &'static [(&'static str, i64)],
    /// The harness skips hooks it has not been told to trust, keyed on their position in
    /// `file` (Codex): a sync that moves or adds one says so.
    pub trust_gated: bool,
}

/// How a target root takes a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The hook's events are merged into the target's hook config file.
    Merge(MergeFile),
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
    /// Generated TypeScript for the harness (`hooks::typescript`).
    TypeScript(&'static Harness),
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
    /// The harness version `events` was last checked against; `None` while the rows are
    /// unchecked.
    pub verified_against: Option<&'static str>,
    pub rules: Rules,
}

impl Profile {
    /// The profile's row for `name`.
    pub fn event(&self, name: &str) -> Option<&'static Event> {
        self.events.iter().find(|e| e.name == name)
    }

    /// Whether the rules render `[hooks]` at all.
    pub fn renders(&self) -> bool {
        !matches!(self.rules, Rules::Withheld { .. })
    }
}

/// One target root's hook rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub root: &'static str,
    pub placement: Placement,
    pub profile: Profile,
    /// Events the harness reads that `profile.events` has no row for: they travel only in a
    /// fragment marked `unchecked`.
    pub other_events: &'static [&'static str],
}

/// Where a file-placement target's generated file gets its suffix: the hook name ends in
/// neither, so a generated file never collides with an authored one.
const GENERATED_SUFFIX: &str = super::RESERVED_NAME_SUFFIX;

impl Target {
    /// Whether the harness reads `event` from its hook file; it ignores any other key.
    pub fn reads(&self, event: &str) -> bool {
        self.profile.event(event).is_some() || self.other_events.contains(&event)
    }

    /// The file a merge target appends entries to; `None` for a file target.
    pub fn merge_file(&self) -> Option<&MergeFile> {
        match &self.placement {
            Placement::Merge(merge) => Some(merge),
            Placement::File { .. } => None,
        }
    }

    /// What is wrong with a file target's fragment, if anything. Warning-only: a file
    /// fragment is otherwise opaque, and the caller words the diagnostic.
    pub fn fragment_problem(&self, content: &str) -> Option<&'static str> {
        match self.profile.rules {
            Rules::TypeScript(harness) => harness.fragment_problem(content),
            _ => None,
        }
    }

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

// Claude Code 2.1.312 (`CLAUDE_VERSION`); event names from https://code.claude.com/docs/en/hooks (2026-07-24),
// matcher fields from research/claude-codex-cursor-answers.md § Matchers per moment.
const CLAUDE_EVENTS: &[Event] = &[
    matched("SessionStart", "source"),
    event("Setup"),
    event("UserPromptSubmit").timeout_ms(30_000),
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
    matched("SessionEnd", "reason").timeout_ms(1_500),
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

// What the generated OpenCode plugin registers (verified live on OpenCode 2.2.0, `matcher`
// fields included). `PreCompact` has no matcher field: its compaction callback does not say
// why it ran.
const OPENCODE_EVENTS: &[Event] = &[
    matched("PreToolUse", "tool_name"),
    matched("PostToolUse", "tool_name"),
    matched("PostToolUseFailure", "tool_name"),
    event("UserPromptSubmit"),
    event("PreCompact"),
    matched("PermissionRequest", "tool_name"),
];

// What the generated Pi extension registers (Pi 1.4.0).
const PI_EVENTS: &[Event] = &[
    matched("PreToolUse", "tool_name"),
    matched("PostToolUse", "tool_name"),
    matched("PostToolUseFailure", "tool_name"),
    event("UserPromptSubmit"),
    matched("SessionStart", "source"),
    event("Stop"),
    matched("PreCompact", "trigger"),
    matched("PostCompact", "trigger"),
    matched("SessionEnd", "reason"),
];

const CLAUDE_VERSION: &str = "Claude Code 2.1.312";

/// What the Claude plugin tree's `hooks/hooks.json` must satisfy: Claude's events under
/// Codex's handler rules, because Claude, Codex and Grok all read that one file (D21).
pub const PLUGIN_CLAUDE_TREE: Profile = Profile {
    events: CLAUDE_EVENTS,
    verified_against: Some(CLAUDE_VERSION),
    rules: Rules::Codex,
};

const TARGETS: &[Target] = &[
    Target {
        root: ".claude",
        // Hook commands carry machine-local cache paths, so they belong in the gitignored
        // `settings.local.json` rather than the committed `settings.json`.
        placement: Placement::Merge(MergeFile {
            file: "settings.local.json",
            top_level: &[],
            trust_gated: false,
        }),
        profile: Profile {
            events: CLAUDE_EVENTS,
            verified_against: Some(CLAUDE_VERSION),
            rules: Rules::Claude,
        },
        other_events: &[],
    },
    Target {
        root: ".codex",
        placement: Placement::Merge(MergeFile {
            file: "hooks.json",
            top_level: &[],
            trust_gated: true,
        }),
        profile: Profile {
            events: CODEX_EVENTS,
            verified_against: Some("codex-cli 0.164.0"),
            rules: Rules::Codex,
        },
        other_events: &["PreModelRequest", "PostModelRequest", "Interrupt"],
    },
    Target {
        root: ".cursor",
        // Cursor's `hooks.json` is a flat entry array and requires the version wrapper.
        placement: Placement::Merge(MergeFile {
            file: "hooks.json",
            top_level: &[("version", 1)],
            trust_gated: false,
        }),
        profile: Profile {
            events: CURSOR_EVENTS,
            verified_against: Some("cursor-agent 2026.07.16"),
            rules: Rules::Withheld {
                reason: "mars renders no generated hooks for Cursor before 0.26",
            },
        },
        other_events: &[],
    },
    Target {
        root: ".opencode",
        placement: Placement::File { dir: "plugins" },
        profile: Profile {
            events: OPENCODE_EVENTS,
            verified_against: Some("OpenCode 2.2.0"),
            rules: Rules::TypeScript(&typescript::OPENCODE),
        },
        other_events: &[],
    },
    Target {
        root: ".pi",
        placement: Placement::File { dir: "extensions" },
        profile: Profile {
            events: PI_EVENTS,
            verified_against: Some("Pi 1.4.0"),
            rules: Rules::TypeScript(&typescript::PI),
        },
        other_events: &[],
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
