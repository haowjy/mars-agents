//! Explicit per-harness native tool names for Mars capabilities.
//!
//! One table per harness, one row per capability. Nothing is derived by naming convention: a
//! row names the harness's real tools (one capability can cover several), says the harness has
//! no such tool ([`Native::Absent`]), says it does the thing without a tool to switch
//! ([`Native::NoSwitch`], with the reason), or is missing, which means Mars has not decided for
//! that harness. Both `NoSwitch` and a missing row report as `unknown`, a gap Meridian
//! surfaces, because calling an unenforceable deny `absent` would report it as met. A
//! `NoSwitch` row is a settled fact and carries its reason in the bundle's `note`; a missing
//! row says Mars has not decided.
//! [`Native::native_tools`] always returns the unscoped harness tool set. It stays distinct
//! from a scoped permission spelling such as Claude `Edit(path)` for the `Write` tool.
//!
//! Every name below must appear in the harness's real tool list, which the tests pin with the
//! source each list came from. When a harness ships a tool, add it to that list first.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::HarnessId;

/// What a harness calls a capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    /// The harness's tools for this capability. A deny or allow covers all of them.
    Tools(&'static [&'static str]),
    /// A harness uses different permission names once a rule has a scope. Claude's file tools
    /// are the motivating case: bare `Write` gates that tool, but only `Edit(path)` gates a
    /// path for every built-in file editor.
    ScopedTools {
        unscoped: &'static [&'static str],
        scoped: &'static [&'static str],
        note: &'static str,
    },
    /// The harness has no such tool, so there is nothing to allow or deny.
    Absent,
    /// The harness can do the thing but offers no tool to switch it off (Codex and Pi load
    /// skills by reading `SKILL.md`; Codex reads, globs and greps through its shell). The
    /// string says why, for the bundle's `note`.
    NoSwitch(&'static str),
}

impl Native {
    /// The unscoped harness tool names this permission row concerns. Scoped permission names can
    /// differ from these tools, but availability controls always use the unscoped tool set.
    pub fn native_tools(self) -> &'static [&'static str] {
        match self {
            Self::Tools(names)
            | Self::ScopedTools {
                unscoped: names, ..
            } => names,
            Self::Absent | Self::NoSwitch(_) => &[],
        }
    }
}

use Native::{Absent, NoSwitch, ScopedTools, Tools};

type Table = &'static [(&'static str, Native)];

/// The row for `capability` on `harness`, or `None` when Mars has no entry.
pub fn lookup(capability: &str, harness: HarnessId) -> Option<Native> {
    table(harness)
        .iter()
        .find(|(name, _)| *name == capability)
        .map(|(_, native)| *native)
}

/// What a consumer reads of the tool vocabulary: every capability and, per host, what the
/// host calls it. Derived only from the tables in this file.
#[derive(Debug, Clone, Serialize)]
pub struct Vocabulary {
    pub capabilities: Vec<crate::tool_names::CapabilityEntry>,
    pub natives: BTreeMap<&'static str, BTreeMap<&'static str, NativeEntry>>,
}

/// One row of a host's table, in the published shape.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NativeEntry {
    Tools {
        tools: &'static [&'static str],
    },
    ScopedTools {
        unscoped: &'static [&'static str],
        scoped: &'static [&'static str],
        note: &'static str,
    },
    Absent,
    NoSwitch {
        note: &'static str,
    },
}

impl From<Native> for NativeEntry {
    fn from(native: Native) -> Self {
        match native {
            Tools(tools) => Self::Tools { tools },
            ScopedTools {
                unscoped,
                scoped,
                note,
            } => Self::ScopedTools {
                unscoped,
                scoped,
                note,
            },
            Absent => Self::Absent,
            NoSwitch(note) => Self::NoSwitch { note },
        }
    }
}

/// The published vocabulary: the capability list plus one native table per harness and Flow.
pub fn vocabulary() -> Vocabulary {
    let rows = |table: Table| {
        table
            .iter()
            .map(|(name, native)| (*name, NativeEntry::from(*native)))
            .collect()
    };
    let mut natives: BTreeMap<_, _> = crate::harness::ALL
        .iter()
        .map(|host| (host.as_str(), rows(table(*host))))
        .collect();
    natives.insert("meridian-flow", rows(FLOW));
    Vocabulary {
        capabilities: crate::tool_names::capabilities(),
        natives,
    }
}

fn table(harness: HarnessId) -> Table {
    match harness {
        HarnessId::Claude => CLAUDE,
        HarnessId::Codex => CODEX,
        HarnessId::OpenCode => OPENCODE,
        HarnessId::Pi => PI,
        HarnessId::Cursor => CURSOR,
    }
}

/// Claude Code 2.1.308.
///
/// Source: the tools and permissions references (code.claude.com/docs/en/tools-reference and
/// code.claude.com/docs/en/permissions) and the 2.1.308
/// binary. Probed live with `claude -p --disallowedTools`: `Agent` and `Task` both remove the
/// sub-agent tool, `Edit` alone leaves `Write` and `NotebookEdit` available, and a misspelled
/// name (`Websearch`) is silently ignored. Bare `edit` therefore names all three file-editing
/// tools. Scoped file rules are different: Claude consults only `Edit(path)` and `Read(path)`,
/// not `Write(path)`, `NotebookEdit(path)`, `Glob(path)`, or `Grep(path)`. Direct 2.1.308 probes
/// confirmed the documented behavior. `WebFetch(domain:...)` is a valid scoped rule.
/// `agent` is the sub-agent family, as it is on Codex: `Agent` spawns, `SendMessage` and
/// `ListAgents` reach the agents it spawned, and an `agent: deny` that left them available would
/// still let the agent steer sub-agents.
const CLAUDE: Table = &[
    ("bash", Tools(&["Bash"])),
    ("read", Tools(&["Read"])),
    // Claude lists through Bash; there is no listing tool.
    ("ls", Absent),
    ("work", Absent),
    (
        "write",
        ScopedTools {
            unscoped: &["Write"],
            scoped: &["Edit"],
            note: "Claude scopes Write through Edit(path): an allow adds no practical write power because Write can replace the whole file, while a deny also refuses other built-in file edits at that path",
        },
    ),
    (
        "edit",
        ScopedTools {
            unscoped: &["Edit", "Write", "NotebookEdit"],
            scoped: &["Edit"],
            note: "Claude uses Edit(path) to scope every built-in file-editing tool",
        },
    ),
    (
        "notebook",
        ScopedTools {
            unscoped: &["NotebookEdit"],
            scoped: &["Edit"],
            note: "Claude scopes NotebookEdit through Edit(path), which also covers other built-in file edits at that path",
        },
    ),
    ("agent", Tools(&["Agent", "SendMessage", "ListAgents"])),
    ("skill", Tools(&["Skill"])),
    ("workflow", Tools(&["Workflow"])),
    (
        "glob",
        ScopedTools {
            unscoped: &["Glob"],
            scoped: &["Read"],
            note: "Claude scopes Glob paths through Read(path), which also covers other built-in file readers at that path",
        },
    ),
    (
        "grep",
        ScopedTools {
            unscoped: &["Grep"],
            scoped: &["Read"],
            note: "Claude scopes Grep paths through Read(path), which also covers other built-in file readers at that path",
        },
    ),
    ("web", Tools(&["WebSearch", "WebFetch"])),
    ("web_search", Tools(&["WebSearch"])),
    ("web_fetch", Tools(&["WebFetch"])),
    ("ask_user", Tools(&["AskUserQuestion"])),
    // `TaskStop` stops background tasks; Meridian has always grouped it with the task list.
    // `TaskOutput` was removed in 2.1.277: 2.1.307 lists it only among retired tool names and
    // offers no such tool, and `--disallowedTools TaskOutput` there is silently ignored. It stays
    // because older Claude builds still offer it and Meridian's platform rule denies it.
    (
        "task",
        Tools(&[
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskOutput",
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
    (
        "image",
        NoSwitch("claude views images through Read; no switch"),
    ),
];

/// Codex 0.163.0.
///
/// Source: the 0.163.0 binary (`core/src/tools/handlers/*` and the tool names it registers).
/// Shell execution is the unified `exec_command`. `write_stdin` only feeds a session
/// `exec_command` opened, so `bash` names `exec_command` alone: Meridian gates Codex's shell
/// with a switch, and denying `exec_command` leaves no session for `write_stdin` to feed.
/// `agent` names both multi-agent tool sets the binary registers: v1
/// (`multi_agents/`: spawn_agent, send_input, resume_agent, wait_agent, close_agent) and v2
/// (`multi_agents_v2/`: spawn_agent, send_message, followup_task, wait_agent, interrupt_agent,
/// list_agents); the model's `multi_agent_version` picks which one a session gets. Codex has no
/// notebook tool. It has no file-read, glob or grep tool either, but it does all three through
/// the shell (`NoSwitch`). `web` is Codex's hosted `web_search`, which also opens and searches
/// within pages (`OpenPage`, `FindInPage` actions), so `web_fetch` has no tool of its own.
/// `skill` loads by reading `SKILL.md`; the `skills.list` / `skills.read` tools of the skills
/// extension are not something Mars can name. `update_plan` is the task list ("Updates the task
/// plan.").
const CODEX: Table = &[
    ("bash", Tools(&["exec_command"])),
    ("ls", Absent),
    ("work", Absent),
    (
        "read",
        NoSwitch("codex reads files through its shell; no switch"),
    ),
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
    (
        "skill",
        NoSwitch("codex loads skills by reading files; no switch"),
    ),
    ("workflow", Absent),
    (
        "glob",
        NoSwitch("codex finds files through its shell; no switch"),
    ),
    (
        "grep",
        NoSwitch("codex searches files through its shell; no switch"),
    ),
    ("web", Tools(&["web_search"])),
    ("web_search", Tools(&["web_search"])),
    (
        "web_fetch",
        NoSwitch("codex opens pages through hosted web_search; no separate switch"),
    ),
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
    ("image", Tools(&["view_image"])),
];

/// OpenCode, in the v1.19.3 permission vocabulary. Meridian knows the running version and
/// translates to v2 spellings (`bash` → `shell`, `task` → `subagent`, `write` → `edit`).
/// Mars does the same for plugin agents in [`opencode_v2_action`], so keep the two in step.
///
/// Source: opencode.ai/docs/permissions, the provider tool lists captured from v1.19.3 and
/// v2.0.29 (`opencode-matrix.md`), and strings from both binaries. `edit` is the permission
/// that covers `edit`, `write` and `patch`, so `write` has no name of its own. The task list
/// is `todowrite` alone: `todoread` is in neither binary. `plan_enter` / `plan_exit` are v1.19.3
/// permissions (its agent defaults deny them); v2.0.29 has no such names, where a rule for
/// them is inert. OpenCode has no notebook, cron, monitor or tool-search tool.
const OPENCODE: Table = &[
    ("bash", Tools(&["bash"])),
    ("ls", Tools(&["list"])),
    ("work", Absent),
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
    (
        "image",
        NoSwitch("opencode views images through read; no switch"),
    ),
];

/// The v2 spelling of a v1.19.3 OpenCode permission key, for a consumer that skips OpenCode's
/// own agent-file loader (which does this translation): the plugin `ctx.agent` API takes v2
/// rule actions. Keys v2 spells the same pass through.
pub fn opencode_v2_action(v1: &str) -> &str {
    match v1 {
        "bash" => "shell",
        "write" | "patch" => "edit",
        "task" => "subagent",
        other => other,
    }
}

/// Pi 1.1.0 / 1.2.0, built-in tools only.
///
/// Source: `pi --help` and the installed package's tool registry
/// (`dist/core/tools/index.js`: bash, edit, find, grep, ls, powershell, read, write). Pi
/// grows by extensions, which Mars cannot see: `absent` means "no built-in tool", and
/// Meridian's own extensions register tools (`bash_manage`) that no capability names. Pi's
/// `edit` and `write` are separate tools, so the `edit` capability covers both. Pi has skills
/// (`--skill`, `--no-skills`) but they load by reading `SKILL.md`, with no tool to switch
/// (`NoSwitch`).
const PI: Table = &[
    ("bash", Tools(&["bash"])),
    ("ls", Tools(&["ls"])),
    ("work", Absent),
    ("read", Tools(&["read"])),
    ("write", Tools(&["write"])),
    ("edit", Tools(&["edit", "write"])),
    ("notebook", Absent),
    ("agent", Absent),
    (
        "skill",
        NoSwitch("pi loads skills by reading files; no switch"),
    ),
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
    ("image", NoSwitch("pi views images through read; no switch")),
];

/// Cursor. Unmaintained: these rows are carried over unchanged from Mars 0.16's
/// overrides and PascalCase convention, not re-verified against a current Cursor. Cursor
/// `edit` has always named only `StrReplace`, so it does not cover `Write`. Anything the old
/// convention guessed beyond these rows is now `unknown`.
const CURSOR: Table = &[
    ("bash", Tools(&["Shell"])),
    ("ls", Absent),
    ("work", Absent),
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

/// Meridian Flow's own tools (mars-agents#207). Flow is a consumer, not a `HarnessId`, so
/// its table is published in the vocabulary but never used to lower a native file. `search`
/// is Flow's name for `grep`; `ask_user` maps to `ask`, which is not a capability alias because
/// `ask` is a tool-policy action word.
const FLOW: Table = &[
    ("read", Tools(&["read"])),
    ("write", Tools(&["write"])),
    ("edit", Tools(&["write"])),
    ("grep", Tools(&["grep"])),
    ("glob", Tools(&["glob"])),
    ("ls", Tools(&["ls"])),
    ("skill", Tools(&["skill"])),
    ("ask_user", Tools(&["ask"])),
    (
        "agent",
        Tools(&[
            "spawn",
            "thread_message",
            "thread_report",
            "thread_ls",
            "thread_history",
        ]),
    ),
    ("work", Tools(&["work"])),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool each harness ships, with where the list came from. A name in a table above
    /// that is not here is a guess.
    fn real_tools(harness: HarnessId) -> &'static [&'static str] {
        match harness {
            // Tools reference + 2.1.307 binary, plus `TaskOutput`: removed in 2.1.277, so 2.1.307
            // no longer offers it, but older builds do (see the `task` row).
            HarnessId::Claude => &[
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
                "TaskOutput",
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
            HarnessId::Codex => &[
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
            HarnessId::OpenCode => &[
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
            HarnessId::Pi => &[
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
            HarnessId::Cursor => &[
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
    fn every_native_name_and_native_tool_is_a_real_tool_of_its_harness() {
        for &harness in crate::harness::ALL {
            let real = real_tools(harness);
            for (capability, native) in table(harness) {
                let lists: &[&[&str]] = match native {
                    Tools(names) => &[names],
                    ScopedTools {
                        unscoped, scoped, ..
                    } => &[unscoped, scoped],
                    Absent | NoSwitch(_) => &[],
                };
                for names in lists {
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
                assert!(
                    native.native_tools().iter().all(|name| real.contains(name)),
                    "{harness:?} `{capability}` has a native tool outside its real tool list"
                );
            }
        }
    }

    #[test]
    fn claude_scoped_file_rules_separate_permission_names_from_native_tools() {
        for (capability, unscoped, scoped) in [
            ("write", &["Write"][..], &["Edit"][..]),
            (
                "edit",
                &["Edit", "Write", "NotebookEdit"][..],
                &["Edit"][..],
            ),
            ("notebook", &["NotebookEdit"][..], &["Edit"][..]),
            ("glob", &["Glob"][..], &["Read"][..]),
            ("grep", &["Grep"][..], &["Read"][..]),
        ] {
            let Some(ScopedTools {
                unscoped: actual_unscoped,
                scoped: actual_scoped,
                note,
            }) = lookup(capability, HarnessId::Claude)
            else {
                panic!("Claude `{capability}` must distinguish bare and scoped rules");
            };
            assert_eq!(actual_unscoped, unscoped, "{capability} bare rule");
            assert_eq!(actual_scoped, scoped, "{capability} scoped rule");
            assert_eq!(
                lookup(capability, HarnessId::Claude)
                    .expect("row")
                    .native_tools(),
                unscoped,
                "{capability} availability tools"
            );
            assert!(
                !note.is_empty(),
                "{capability} must explain the wider category"
            );
        }

        assert_eq!(
            lookup("web_fetch", HarnessId::Claude),
            Some(Tools(&["WebFetch"]))
        );
    }

    #[test]
    fn every_row_is_a_canonical_capability_named_once() {
        for &harness in crate::harness::ALL {
            let mut seen = std::collections::HashSet::new();
            for (capability, _) in table(harness) {
                assert!(
                    crate::tool_names::is_canonical_capability(capability),
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
        // A missing row means Mars has not decided. The maintained harnesses decide every
        // capability: tools, `Absent`, or `NoSwitch`.
        for harness in [
            HarnessId::Claude,
            HarnessId::Codex,
            HarnessId::OpenCode,
            HarnessId::Pi,
        ] {
            for capability in crate::tool_names::canonical_capabilities() {
                assert!(
                    lookup(capability, harness).is_some(),
                    "{harness:?} has no row for `{capability}`"
                );
            }
        }
    }
}
