//! Explicit per-harness native tool names for Mars capabilities.
//!
//! One table per harness, one row per capability. Nothing is derived by naming convention:
//! a row names the harness's real tools (one capability can cover several), says the
//! harness has no such tool ([`Native::Absent`]), or is missing, which Mars reports as a gap
//! (`unknown`) instead of guessing.
//!
//! `Absent` is for a harness that cannot do the thing at all. When it can but offers no tool to
//! switch it (Codex and Pi load skills by reading `SKILL.md`; Codex fetches pages through its
//! `web_search` tool), leave the row out. A deny is then `unknown`, a gap Meridian reports:
//! calling it `absent` would report an unenforced deny as satisfied.
//!
//! Every name below must appear in the harness's real tool list, which the tests pin with
//! the source each list came from. When a harness ships a tool, add it to that list first.

use crate::compiler::agents::HarnessKind;

/// What a harness calls a capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Native {
    /// The harness's tools for this capability. A deny or allow covers all of them.
    Tools(&'static [&'static str]),
    /// Like `Tools`, but a rule's pattern scopes only the first tool. The rest continue what it
    /// started and take no pattern of their own, so a scoped rule leaves them out.
    ToolsScopedToFirst(&'static [&'static str]),
    /// The harness has no such tool, so there is nothing to allow or deny.
    Absent,
}

use Native::{Absent, Tools, ToolsScopedToFirst};

type Table = &'static [(&'static str, Native)];

/// The row for `capability` on `harness`, or `None` when Mars has no entry.
pub(crate) fn lookup(capability: &str, harness: HarnessKind) -> Option<Native> {
    table(harness)
        .iter()
        .find(|(name, _)| *name == capability)
        .map(|(_, native)| *native)
}

fn table(harness: HarnessKind) -> Table {
    match harness {
        HarnessKind::Claude => CLAUDE,
        HarnessKind::Codex => CODEX,
        HarnessKind::OpenCode => OPENCODE,
        HarnessKind::Pi => PI,
        HarnessKind::Cursor => CURSOR,
    }
}

/// Claude Code 2.1.307.
///
/// Source: the tools reference (code.claude.com/docs/en/tools-reference) and the 2.1.307
/// binary. Probed live with `claude -p --disallowedTools`: `Agent` and `Task` both remove the
/// sub-agent tool, `Edit` alone leaves `Write` and `NotebookEdit` available, and a misspelled
/// name (`Websearch`) is silently ignored. So `edit` names all three file-editing tools.
const CLAUDE: Table = &[
    ("bash", Tools(&["Bash"])),
    ("read", Tools(&["Read"])),
    ("write", Tools(&["Write"])),
    ("edit", Tools(&["Edit", "Write", "NotebookEdit"])),
    ("notebook", Tools(&["NotebookEdit"])),
    ("agent", Tools(&["Agent"])),
    ("skill", Tools(&["Skill"])),
    ("workflow", Tools(&["Workflow"])),
    ("glob", Tools(&["Glob"])),
    ("grep", Tools(&["Grep"])),
    ("web", Tools(&["WebSearch", "WebFetch"])),
    ("web_search", Tools(&["WebSearch"])),
    ("web_fetch", Tools(&["WebFetch"])),
    ("ask_user", Tools(&["AskUserQuestion"])),
    // `TaskStop` stops background tasks; Meridian has always grouped it with the task list.
    // `TaskOutput` is gone since 2.1.277, and naming a removed tool only draws a warning.
    (
        "task",
        Tools(&[
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskUpdate",
            "TaskStop",
            "TodoWrite",
        ]),
    ),
    ("cron", Tools(&["CronCreate", "CronDelete", "CronList"])),
    ("notifications", Tools(&["PushNotification"])),
    ("plan_mode", Tools(&["EnterPlanMode", "ExitPlanMode"])),
    ("worktree", Tools(&["EnterWorktree", "ExitWorktree"])),
    ("lsp", Tools(&["LSP"])),
    ("monitor", Tools(&["Monitor"])),
    ("send_user_file", Tools(&["SendUserFile"])),
    ("schedule_wakeup", Tools(&["ScheduleWakeup"])),
    ("remote_trigger", Tools(&["RemoteTrigger"])),
    ("tool_search", Tools(&["ToolSearch"])),
];

/// Codex 0.163.0.
///
/// Source: the 0.163.0 binary (`core/src/tools/handlers/*` and the tool names it registers).
/// Shell execution is the unified `exec_command`, and `write_stdin` feeds the session it opened,
/// so `bash` names both. A command pattern (`bash(git push *)`) matches the command
/// `exec_command` starts; `write_stdin` carries only input, so a scoped rule leaves it out. `agent` names both multi-agent tool sets the binary registers: v1
/// (`multi_agents/`: spawn_agent, send_input, resume_agent, wait_agent, close_agent) and v2
/// (`multi_agents_v2/`: spawn_agent, send_message, followup_task, wait_agent, interrupt_agent,
/// list_agents); the model's `multi_agent_version` picks which one a session gets. Codex has no
/// file-read, glob, grep or notebook tool: the model reads files through the shell. `web` is
/// Codex's hosted `web_search`, which also opens and searches within pages (`OpenPage`,
/// `FindInPage` actions), so `web_fetch` has no tool of its own to switch and is left out
/// (`unknown`). `skill` is left out too: local skills load by reading `SKILL.md`, and the
/// `skills.list` / `skills.read` tools of the skills extension are not something Mars can name.
/// `update_plan` is the task list ("Updates the task plan.").
const CODEX: Table = &[
    ("bash", ToolsScopedToFirst(&["exec_command", "write_stdin"])),
    ("read", Absent),
    ("write", Tools(&["apply_patch"])),
    ("edit", Tools(&["apply_patch"])),
    ("notebook", Absent),
    (
        "agent",
        Tools(&[
            "spawn_agent",
            "send_input",
            "resume_agent",
            "wait_agent",
            "close_agent",
            "send_message",
            "followup_task",
            "interrupt_agent",
            "list_agents",
        ]),
    ),
    ("workflow", Absent),
    ("glob", Absent),
    ("grep", Absent),
    ("web", Tools(&["web_search"])),
    ("web_search", Tools(&["web_search"])),
    ("ask_user", Tools(&["request_user_input"])),
    ("task", Tools(&["update_plan"])),
    ("cron", Absent),
    ("notifications", Absent),
    ("plan_mode", Absent),
    ("worktree", Absent),
    ("lsp", Absent),
    ("monitor", Absent),
    ("send_user_file", Absent),
    ("schedule_wakeup", Absent),
    ("remote_trigger", Absent),
    ("tool_search", Tools(&["tool_search"])),
];

/// OpenCode, in the v1.19.3 permission vocabulary. Meridian knows the running version and
/// translates to v2 spellings (`bash` → `shell`, `task` → `subagent`, `write` → `edit`).
///
/// Source: opencode.ai/docs/permissions, the provider tool lists captured from v1.19.3 and
/// v2.0.29 (`opencode-matrix.md`), and strings from both binaries. `edit` is the permission
/// that covers `edit`, `write` and `patch`, so `write` has no name of its own. The task list
/// is `todowrite` alone: `todoread` is in neither binary. `plan_enter` / `plan_exit` are v1.19.3
/// permissions (its agent defaults deny them); v2.0.29 has no such names, where a rule for
/// them is inert. OpenCode has no notebook, cron, monitor or tool-search tool.
const OPENCODE: Table = &[
    ("bash", Tools(&["bash"])),
    ("read", Tools(&["read"])),
    ("write", Tools(&["edit"])),
    ("edit", Tools(&["edit"])),
    ("notebook", Absent),
    ("agent", Tools(&["task"])),
    ("skill", Tools(&["skill"])),
    ("workflow", Absent),
    ("glob", Tools(&["glob"])),
    ("grep", Tools(&["grep"])),
    ("web", Tools(&["websearch", "webfetch"])),
    ("web_search", Tools(&["websearch"])),
    ("web_fetch", Tools(&["webfetch"])),
    ("ask_user", Tools(&["question"])),
    ("task", Tools(&["todowrite"])),
    ("plan_mode", Tools(&["plan_enter", "plan_exit"])),
    ("cron", Absent),
    ("notifications", Absent),
    ("worktree", Absent),
    ("lsp", Tools(&["lsp"])),
    ("monitor", Absent),
    ("send_user_file", Absent),
    ("schedule_wakeup", Absent),
    ("remote_trigger", Absent),
    ("tool_search", Absent),
];

/// Pi 1.1.0 / 1.2.0, built-in tools only.
///
/// Source: `pi --help` and the installed package's tool registry
/// (`dist/core/tools/index.js`: bash, edit, find, grep, ls, powershell, read, write). Pi
/// grows by extensions, which Mars cannot see: `absent` means "no built-in tool", and
/// Meridian's own extensions register tools (`bash_manage`) that no capability names. Pi's
/// `edit` and `write` are separate tools, so the `edit` capability covers both. `skill` is left
/// out (`unknown`): Pi has skills (`--skill`, `--no-skills`) but they load by reading `SKILL.md`,
/// with no tool to switch.
const PI: Table = &[
    ("bash", Tools(&["bash"])),
    ("read", Tools(&["read"])),
    ("write", Tools(&["write"])),
    ("edit", Tools(&["edit", "write"])),
    ("notebook", Absent),
    ("agent", Absent),
    ("workflow", Absent),
    ("glob", Tools(&["find"])),
    ("grep", Tools(&["grep"])),
    ("web", Absent),
    ("web_search", Absent),
    ("web_fetch", Absent),
    ("ask_user", Absent),
    ("task", Absent),
    ("plan_mode", Absent),
    ("cron", Absent),
    ("notifications", Absent),
    ("worktree", Absent),
    ("lsp", Absent),
    ("monitor", Absent),
    ("send_user_file", Absent),
    ("schedule_wakeup", Absent),
    ("remote_trigger", Absent),
    ("tool_search", Absent),
];

/// Cursor. Unmaintained: these rows are carried over unchanged from Mars 0.16's
/// overrides and PascalCase convention, not re-verified against a current Cursor. Cursor
/// `edit` has always named only `StrReplace`, so it does not cover `Write`. Anything the old
/// convention guessed beyond these rows is now `unknown`.
const CURSOR: Table = &[
    ("bash", Tools(&["Shell"])),
    ("read", Tools(&["Read"])),
    ("write", Tools(&["Write"])),
    ("edit", Tools(&["StrReplace"])),
    ("notebook", Tools(&["EditNotebook"])),
    ("agent", Tools(&["Task"])),
    ("skill", Tools(&["Skill"])),
    ("glob", Tools(&["Glob"])),
    ("grep", Tools(&["Grep"])),
    ("web", Tools(&["WebSearch", "WebFetch"])),
    ("web_search", Tools(&["WebSearch"])),
    ("web_fetch", Tools(&["WebFetch"])),
    ("ask_user", Tools(&["AskQuestion"])),
    ("task", Tools(&["TodoWrite"])),
    ("plan_mode", Tools(&["SwitchMode"])),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool each harness ships, with where the list came from. A name in a table above
    /// that is not here is a guess.
    fn real_tools(harness: HarnessKind) -> &'static [&'static str] {
        match harness {
            // Tools reference + 2.1.307 binary. Excludes `TaskOutput` (removed 2.1.277).
            HarnessKind::Claude => &[
                "Agent",
                "Artifact",
                "AskUserQuestion",
                "Bash",
                "CronCreate",
                "CronDelete",
                "CronList",
                "DesignSync",
                "Edit",
                "EndConversation",
                "EnterPlanMode",
                "EnterWorktree",
                "ExitPlanMode",
                "ExitWorktree",
                "Glob",
                "Grep",
                "ListAgents",
                "ListMcpResourcesTool",
                "LSP",
                "Monitor",
                "NotebookEdit",
                "PowerShell",
                "PushNotification",
                "Read",
                "ReadMcpResourceTool",
                "RemoteTrigger",
                "ReportFindings",
                "ScheduleWakeup",
                "SendFeedback",
                "SendMessage",
                "SendUserFile",
                "ShareOnboardingGuide",
                "Skill",
                "SubagentHandback",
                "TaskCreate",
                "TaskGet",
                "TaskList",
                "TaskStop",
                "TaskUpdate",
                "TodoWrite",
                "ToolSearch",
                "WaitForMcpServers",
                "WebFetch",
                "WebSearch",
                "Workflow",
                "Write",
            ],
            // 0.163.0 binary: tool handlers (`core/src/tools/handlers/`) and registered tool
            // names. Leaves out experimental and internal tools no capability names (`sleep`,
            // `current_time`, `skills.list`, `skills.read`, memory and message-board tools).
            HarnessKind::Codex => &[
                "apply_patch",
                "close_agent",
                "exec_command",
                "followup_task",
                "image_generation",
                "interrupt_agent",
                "js_repl",
                "list_agents",
                "list_mcp_resource_templates",
                "list_mcp_resources",
                "read_mcp_resource",
                "request_permissions",
                "request_user_input",
                "resume_agent",
                "send_input",
                "send_message",
                "spawn_agent",
                "tool_search",
                "update_plan",
                "view_image",
                "wait_agent",
                "web_search",
                "write_stdin",
            ],
            // v1 permission names: opencode.ai/docs/permissions, v1.19.3 / v2.0.29 captures and
            // binaries.
            HarnessKind::OpenCode => &[
                "bash",
                "edit",
                "glob",
                "grep",
                "list",
                "lsp",
                "plan_enter",
                "plan_exit",
                "question",
                "read",
                "skill",
                "task",
                "todowrite",
                "webfetch",
                "websearch",
            ],
            // Package `@earendil-works/pi-coding-agent` dist/core/tools/index.js.
            HarnessKind::Pi => &[
                "bash",
                "edit",
                "find",
                "grep",
                "ls",
                "powershell",
                "read",
                "write",
            ],
            // Mars 0.16 table; not re-verified (target unmaintained).
            HarnessKind::Cursor => &[
                "AskQuestion",
                "EditNotebook",
                "Glob",
                "Grep",
                "Read",
                "Shell",
                "Skill",
                "StrReplace",
                "SwitchMode",
                "Task",
                "TodoWrite",
                "WebFetch",
                "WebSearch",
                "Write",
            ],
        }
    }

    #[test]
    fn every_native_name_is_a_real_tool_of_its_harness() {
        for &harness in HarnessKind::all() {
            let real = real_tools(harness);
            for (capability, native) in table(harness) {
                if let Tools(names) | ToolsScopedToFirst(names) = native {
                    assert!(
                        !names.is_empty(),
                        "{harness:?} `{capability}` names no tool"
                    );
                    for name in *names {
                        assert!(
                            real.contains(name),
                            "{harness:?} `{capability}` maps to `{name}`, which is not in its real tool list"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_row_is_a_canonical_capability_named_once() {
        for &harness in HarnessKind::all() {
            let mut seen = std::collections::HashSet::new();
            for (capability, _) in table(harness) {
                assert!(
                    crate::compiler::tool_names::is_canonical_capability(capability),
                    "{harness:?} has a row for `{capability}`, which is not a canonical capability"
                );
                assert!(
                    seen.insert(*capability),
                    "{harness:?} repeats `{capability}`"
                );
            }
        }
    }

    #[test]
    fn maintained_harnesses_answer_for_every_canonical_capability() {
        // A missing row reports a gap, so a maintained harness says `absent` explicitly
        // rather than stay silent. A deliberate hole is a thing the harness can do with no tool
        // to switch it; the reason is in that harness's table comment.
        let deliberate_gaps: &[(HarnessKind, &str)] = &[
            (HarnessKind::Codex, "skill"),
            (HarnessKind::Codex, "web_fetch"),
            (HarnessKind::Pi, "skill"),
        ];
        for harness in [
            HarnessKind::Claude,
            HarnessKind::Codex,
            HarnessKind::OpenCode,
            HarnessKind::Pi,
        ] {
            for capability in crate::compiler::tool_names::canonical_capabilities() {
                let missing = lookup(capability, harness).is_none();
                let deliberate = deliberate_gaps.contains(&(harness, capability));
                assert_eq!(
                    missing, deliberate,
                    "{harness:?} `{capability}`: missing={missing}, deliberate gap={deliberate}"
                );
            }
        }
    }

    /// The shipped profiles' vocabulary on each harness, written out so a table edit that
    /// changes a name shows up here.
    #[test]
    fn shipped_profile_capabilities_map_as_documented() {
        let expect: &[(HarnessKind, &str, Native)] = &[
            (
                HarnessKind::Claude,
                "edit",
                Tools(&["Edit", "Write", "NotebookEdit"]),
            ),
            (
                HarnessKind::Claude,
                "web",
                Tools(&["WebSearch", "WebFetch"]),
            ),
            (HarnessKind::Claude, "ask_user", Tools(&["AskUserQuestion"])),
            (HarnessKind::Claude, "agent", Tools(&["Agent"])),
            (
                HarnessKind::Codex,
                "bash",
                ToolsScopedToFirst(&["exec_command", "write_stdin"]),
            ),
            (
                HarnessKind::Codex,
                "agent",
                Tools(&[
                    "spawn_agent",
                    "send_input",
                    "resume_agent",
                    "wait_agent",
                    "close_agent",
                    "send_message",
                    "followup_task",
                    "interrupt_agent",
                    "list_agents",
                ]),
            ),
            (HarnessKind::Codex, "read", Absent),
            (HarnessKind::Codex, "edit", Tools(&["apply_patch"])),
            (
                HarnessKind::Codex,
                "ask_user",
                Tools(&["request_user_input"]),
            ),
            (HarnessKind::Codex, "web", Tools(&["web_search"])),
            (
                HarnessKind::OpenCode,
                "web",
                Tools(&["websearch", "webfetch"]),
            ),
            (HarnessKind::OpenCode, "ask_user", Tools(&["question"])),
            (HarnessKind::OpenCode, "agent", Tools(&["task"])),
            (HarnessKind::OpenCode, "task", Tools(&["todowrite"])),
            (
                HarnessKind::OpenCode,
                "plan_mode",
                Tools(&["plan_enter", "plan_exit"]),
            ),
            (HarnessKind::Pi, "glob", Tools(&["find"])),
            (HarnessKind::Pi, "ask_user", Absent),
        ];
        for (harness, capability, native) in expect {
            assert_eq!(
                lookup(capability, *harness),
                Some(*native),
                "{harness:?} `{capability}`"
            );
        }
    }
}
