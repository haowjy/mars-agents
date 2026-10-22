# Agent Compilation

During `mars sync`, every agent is compiled to a canonical full-fidelity
artifact in `.mars/agents/`. Native harness artifacts
(`.claude/agents/`, `.codex/agents/`, etc.) are emitted when native agent
emission policy allows them. Standalone/default emission lowers every agent to
every configured target harness; Meridian-managed mode suppresses them unless
policy requests all or selective copies through
`[settings.meridian.agent_copy]`.

## The Two Surfaces

Every agent produces a `.mars/agents/<name>.md` artifact regardless of whether `harness:` is set. This is the canonical compiled output: all frontmatter fields preserved, body unchanged. Meridian reads from here at spawn time.

When native emission selects an agent for a harness, a second artifact is emitted in the harness-native directory:

| Target harness | Native artifact |
|---|---|
| `claude` | `.claude/agents/<name>.md` |
| `codex` | `.codex/agents/<name>.toml` |
| `opencode` | `.opencode/agents/<name>.md` |
| `cursor` | `.cursor/agents/<name>.md` |
| `pi` | `.pi/agents/<name>.md` |

Harness-native artifacts are format-translated and field-stripped. They serve as agent discovery surfaces for harness-native invocation (e.g. `codex --agent coder`). Meridian always uses the `.mars/` artifact for its own spawn logic and applies all policy fields through its own projection layer.

Universal agents (no `harness:`) are always installed to `.mars/agents/` and
can be launched by Meridian against any harness. In standalone/default
`EmitAll`, they are also lowered to every configured target harness. Under
Meridian-managed suppression they remain canonical-only unless
`[settings.meridian.agent_copy]` creates a qualifying native copy through the
agent's `model:` alias or, with `include_fanout = true`, its `model-policies`.

## Emission Control

Native artifact emission is controlled by `settings.agent_emission` in `mars.toml`:

| Value | Behavior |
|---|---|
| `auto` (default) | Emit native artifacts in standalone mode; suppress when `MERIDIAN_MANAGED=1` |
| `always` | Always emit native artifacts, even in Meridian-managed mode |
| `never` | Never emit native artifacts |

```toml
[settings]
agent_emission = "always"
```

**`MERIDIAN_MANAGED=1`** — when Meridian invokes mars, it sets this env var. Under `auto`, native artifacts are suppressed: Meridian manages delegation through `.mars/agents/` and `meridian spawn`, and does not want harness-native artifacts competing with that routing. Set `agent_emission = "always"` to emit all native artifacts.

Use `[settings.meridian.agent_copy]` for a selective override under managed mode or `agent_emission = "never"`:

```toml
[settings]
targets = [".claude"]
agent_emission = "never"  # optional

[settings.meridian.agent_copy]
harnesses = ["claude"]
include_fanout = false
```

Each harness in `harnesses` must also have an effective managed target (from
`settings.targets`, or legacy `managed_root` when `targets` is unset). Mars
then emits only agents that qualify for that harness: profiles with a
matching `harness:`, profiles whose `model:` alias resolves to that harness,
and, when `include_fanout = true`, matching `model-policies`. This is the
intentional path for Claude-native `Agent()` copies while Meridian still owns
normal delegation through `.mars/agents/`.

When a native artifact is not emitted because emission is disabled or
selective copy does not qualify it, Mars reconciles all harness shapes in
scope for agents currently in `.mars/agents/`. Removal requires an exact prior
lock ownership record, so changing an agent's `harness:` value does not leave
the previously owned native artifact behind.

## Harness Override Passthrough

Mars does not merge `harness-overrides.<target>` into top-level profile values. Top-level fields are the semantic controls for native lowering, prompt assembly, tool policy, and launch-bundle execution policy.

For launch bundles, Mars selects the `harness-overrides.<resolved-harness>` block and preserves that mapping exactly in `execution_policy.native_config`. The block is target-native passthrough: Mars validates the outer shape and serializability, then leaves nested keys and values alone.

Example: an agent has `effort: low` and `harness-overrides.codex.effort: high`. Codex native lowering still receives `effort: low`; the launch bundle carries `{ "effort": "high" }` under `execution_policy.native_config` for the harness/runtime to interpret.

## Per-Target Field Mapping

### `.mars/agents/<name>.md` (Canonical)

Full fidelity — all source frontmatter preserved after compilation. No fields are stripped. Body unchanged.

### `.claude/agents/<name>.md`

YAML frontmatter + markdown body. Claude Code reads this directly.

| Source field | Claude native | Classification |
|---|---|---|
| `name` | `name:` | exact |
| `description` | `description:` | exact |
| `model` | `model:` | exact |
| `skills` | `skills:` | exact |
| `tools` | `tools:` (+ projected `mcp__…` MCP tokens), only for an allowlist | A list-form `tools:` (or a map with `'*': deny`) lowers exactly: Claude's `tools:` restricts the agent to what it lists. Map-form grants write no `tools:` (an omitted `tools:` inherits every tool, so nothing is lost); `ask` rules and an allowlist with nothing allowed have no field, and Mars records each as dropped lossiness |
| `disallowed-tools` | `disallowed-tools:` (+ projected `mcp__…` MCP tokens) | deny rules projected to Claude's tool names |
| `effort` | `effort:` (`xhigh` → `max`) | exact |
| `approval` | dropped | dropped |
| `sandbox` | dropped | dropped |
| `mode` | dropped | dropped |
| `autocompact` | dropped | meridian-only |
| `autocompact_pct` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `harness-overrides` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |
| `harness` | dropped | dropped |

`approval` and `sandbox` policy fields are applied at launch time by Meridian through its harness projection layer, not stored in the agent file.

### Tool name projection

Mars owns both permission-rule names and unscoped harness tool names. A profile's capability
(`edit`, `web`, `ask_user`, …) becomes both facts through one explicit table per harness
(`src/compiler/tool_natives.rs`, which is authoritative; the table below is a reading copy);
no name is derived by convention. The launch bundle puts permission spellings (including
patterns) in `native` and unscoped availability names in `native_tools`. A capability can cover
several native tools, and the launch bundle reports one of three outcomes per rule:

- `known`: the table names it.
- `absent`: the harness cannot do this at all, so there is nothing to allow or deny.
- `unknown`: Mars cannot name it for this harness. A deny there is not enforced, and `absent`
  would report it as met. The tool is left out of native output, the bundle rule carries an
  empty `native` and a `note` with the reason, and Meridian reports the gap rather than
  guessing around it. The `note` says which of three it is: the harness can do the thing but
  has no tool to switch it (Codex reads, globs and greps through its shell; Codex and Pi load
  skills by reading `SKILL.md`; "codex loads skills by reading files; no switch"), Mars has no
  entry for the tool on this harness, or Mars does not recognize the name (a typo, most
  likely). The bundle adds no warning for any of them: Meridian prints `warnings[]` at every
  launch, so a permission gap travels in `native_status` and `note` alone, and `mars sync`
  is where an author hears about a typo.

The bundle's `*` rule (the `*: deny` of an allowlist) names no tool, so it has empty `native`
and `native_tools` lists and no `native_status`. Absent, unknown and fully shadowed rules also
have an empty `native_tools`; it follows the same `native_status` semantics as `native`.
For a `*: deny` allowlist, consumers build the availability set from known `allow` and `ask`
rules' `native_tools`; `deny` rules name what they concern but grant no availability.

| Capability | Claude | Codex | OpenCode | Pi |
|---|---|---|---|---|
| `bash` | `Bash` | `exec_command` | `bash` | `bash` |
| `read` | `Read` | unknown | `read` | `read` |
| `write` | `Write` | `apply_patch` | `edit` | `write` |
| `edit` | `Edit`, `Write`, `NotebookEdit` | `apply_patch` | `edit` | `edit`, `write` |
| `notebook` | `NotebookEdit` | absent | absent | absent |
| `agent` | `Agent`, `SendMessage`, `ListAgents` | `spawn_agent`, `send_input`, `resume_agent`, `wait_agent`, `close_agent`, `send_message`, `followup_task`, `interrupt_agent`, `list_agents` | `task` | absent |
| `task` | `TaskCreate`, `TaskGet`, `TaskList`, `TaskOutput`¹, `TaskUpdate`, `TaskStop`, `TodoWrite` | `update_plan` | `todowrite` | absent |
| `skill` | `Skill` | unknown | `skill` | unknown |
| `glob` / `grep` | `Glob` / `Grep` | unknown | `glob` / `grep` | `find` / `grep` |
| `web` | `WebSearch`, `WebFetch` | `web_search` (`web_fetch` alone: unknown) | `websearch`, `webfetch` | absent |
| `ask_user` | `AskUserQuestion` | `request_user_input` | `question` | absent |
| `plan_mode` | `EnterPlanMode`, `ExitPlanMode` | absent | `plan_enter`, `plan_exit` | absent |

¹ `TaskOutput` was removed from Claude in 2.1.277, which ignores the name. It stays because
older builds offer it and Meridian denies it with the task list.

The rest of the vocabulary is in `tool_natives.rs`. OpenCode names are the v1 permission
vocabulary; the consumer that knows the running OpenCode version translates to v2 spellings
(`bash` → `shell`, `task` → `subagent`). OpenCode has one `edit` permission for `edit`,
`write` and patch, so `write` and `edit` share a name there. Codex `bash` names `exec_command`
alone: `write_stdin` only feeds a session `exec_command` opened. Each row is pinned by a test
against the harness's real tool list, with the source (docs, binary, probe) written beside the
table. Cursor rows are carried over from earlier releases and not re-verified.

The table shows bare Claude tool rules. Claude uses permission categories for scoped file
rules instead: `write(path)`, `edit(path)` and `notebook(path)` project to `Edit(path)`;
`read(path)`, `glob(path)` and `grep(path)` project to `Read(path)`. Claude accepts
`Write(path)`, `NotebookEdit(path)`, `Glob(path)` and `Grep(path)` spellings but does not
consult them. The separate `native_tools` list still names the callable tools: scoped `write`
names `Write`, scoped `edit` names `Edit`, `Write` and `NotebookEdit`, and scoped `grep` names
`Grep`. The bundle adds a `note` to a scoped rule when the valid category spelling also covers
sibling built-in tools. Bare rules keep the table's names. `WebFetch(domain:...)` is already
Claude's valid scoped spelling and stays unchanged.

MCP refs keep their own projection (below), and `mcp__…` wire tokens pass through verbatim.

**A narrower capability wins.** `edit: deny` with `write: allow` means "no edits, but writing is
fine". On Claude `edit` covers `Write`, so projecting both rules as written would put `Write` in
the allow and the deny list, and a deny beats an allow. Mars resolves this per native name,
before anything is emitted: when two rules with different actions name the same unscoped tool,
the rule whose capability covers fewer tools wins it. Claude's `edit` deny becomes
`Edit` + `NotebookEdit` and `Write` stays allowed. The bundle's `native` and `native_tools`
lists, the flat lists and Claude agent files all carry the resolved names. A harness that
cannot tell two capabilities apart (Codex and OpenCode name one tool for both `write` and
`edit`) stays in conflict: the agent file records a lossiness entry naming the tool that is
both allowed and denied, and the bundle gives each of the two rules a `note` saying the harness
decides.
Scoped Claude rules that collapse to the exact same category permission are resolved by
Claude's documented action order instead: `deny` beats `ask`, which beats `allow`; the losing
rule has empty `native` and `native_tools` lists, and its `note` says why.

**Native agent files cannot hold everything.** An `ask` rule has no field in an agent file,
and neither does an allowlist with nothing allowed (an omitted `tools:` inherits every tool).
A map-form `allow` is a grant, not an allowlist, so it writes no `tools:` field: Claude
would read `tools: [Bash]` as "only Bash". Omitting `tools:` inherits every tool, so the
grant loses nothing and Mars records no lossiness for it.
Mars records an `ask` rule and an empty allowlist as dropped lossiness and never drops them
silently; Meridian enforces them from the launch bundle. Other harnesses' agent files carry no tool list at all, so any
allow, ask or deny rule is recorded as dropped there too.

### `.codex/agents/<name>.toml`

TOML format. Codex reads this for native agent invocation.

| Source field | Codex native key | Classification |
|---|---|---|
| `name` | `name` | exact |
| `description` | `description` | exact |
| `model` | `model` | exact |
| `effort` | `model_reasoning_effort` | exact |
| `sandbox` | `sandbox_mode` | exact |
| `approval` | `approval_policy` | exact |
| body | `developer_instructions` | exact |
| `skills` | dropped | dropped |
| `tools` | dropped | dropped |
| `disallowed-tools` | dropped | dropped |
| `mcp(...)` in `tools` / `disallowed-tools` | dropped (lossiness); enable servers via `mcp/<name>/mcp.toml` | approximate |
| `mode` | dropped | dropped |
| `autocompact` | dropped | meridian-only |
| `autocompact_pct` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |

**Approval value mapping:**

| `approval:` | `approval_policy` |
|---|---|
| `default` | (omitted) |
| `auto` | `"on-request"` |
| `confirm` | `"untrusted"` |
| `yolo` | `"never"` |

**Example output:**

```toml
name = "coder"
description = "Implementation agent for code changes"
model = "gpt55"
model_reasoning_effort = "high"
sandbox_mode = "workspace-write"
approval_policy = "on-request"
developer_instructions = """
# Coder

You turn approved plans into working code.
"""
```

### `.opencode/agents/<name>.md`

YAML frontmatter + markdown body.

| Source field | OpenCode native | Classification |
|---|---|---|
| `name` | `name:` | exact |
| `description` | `description:` | exact |
| `model` | `model:` | exact |
| `mode` | `mode:` | approximate |
| body | body | exact |
| `skills` | dropped | dropped |
| `approval` | dropped | dropped |
| `sandbox` | dropped | dropped |
| `tools` | dropped | dropped |
| `disallowed-tools` | dropped | dropped |
| `effort` | dropped from frontmatter | approximate |
| `mcp(...)` in `tools` / `disallowed-tools` | dropped (lossiness); launch bundle projects OpenCode tokens | approximate |
| `autocompact` | dropped | meridian-only |
| `autocompact_pct` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |

`skills` are not emitted into the OpenCode agent artifact. Skill availability comes from separate skill-surface compilation to `.opencode/skills/`.

`effort` on OpenCode subprocess maps to `--variant`; on streaming/TUI launches it's dropped.

### `.cursor/agents/<name>.md`

YAML frontmatter + markdown body.

| Source field | Cursor native | Classification |
|---|---|---|
| `name` | `name:` | exact |
| `description` | one physical line (`description:` with collapsed whitespace) | exact |
| `model` | `model:` (internally adapted when deterministic mapping exists) | exact |
| `skills` | `skills:` | exact |
| `mode` | `mode:` | approximate |
| body | body | exact |
| `approval` | runtime-only (meridian projects `--force`/`--yolo`) | approximate |
| `sandbox` | runtime-only (meridian projects `--sandbox enabled`/`disabled`) | approximate |
| `tools` | dropped | dropped |
| `disallowed-tools` | dropped | dropped |
| `effort` | dropped from frontmatter | approximate |
| `mcp(...)` in `tools` / `disallowed-tools` | dropped (lossiness); launch bundle projects `Mcp(server:tool)` tokens | approximate |
| `autocompact` | dropped | meridian-only |
| `autocompact_pct` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |

**Approval value mapping:**

| `approval:` | Cursor CLI |
|---|---|
| `default` | (omitted) |
| `auto` | `--force` |
| `confirm` | (omitted — no Cursor equivalent) |
| `yolo` | `--yolo` |

**Sandbox value mapping:**

| `sandbox:` | Cursor CLI |
|---|---|
| `default` | (omitted) |
| `read-only` | `--sandbox enabled` |
| `workspace-write` | `--sandbox disabled` |
| `danger-full-access` | `--sandbox disabled` |

`approval` and `sandbox` are not stored in the Cursor native agent artifact — Cursor IDE doesn't read them from frontmatter. Meridian applies these as runtime CLI flags through its harness projection layer.

Cursor requires a one-line frontmatter description for stable native parsing. Mars normalizes multiline/block descriptions by trimming and collapsing all whitespace to single spaces before emitting `.cursor/agents/*.md`.

Cursor model adaptation is internal to Mars. For common aliases/model IDs, Mars emits deterministic Cursor-native slugs (for example `claude-opus-4-6` → `claude-4.6-opus-high-thinking`, `gpt-5.5` + high effort → `gpt-5.5-high`). If no deterministic mapping exists, Mars preserves the original `model` token.

### `.pi/agents/<name>.md`

YAML frontmatter + markdown body.

| Source field | Pi native | Classification |
|---|---|---|
| `name` | `name:` | exact |
| `description` | `description:` | exact |
| `model` | `model:` | exact |
| `mode` | `mode:` | approximate |
| body | body | exact |
| `effort` | dropped | approximate |
| `mcp(...)` in `tools` / `disallowed-tools` | dropped (lossiness) | approximate |
| All other policy fields | dropped | dropped |
| `autocompact`, `autocompact_pct`, `model-policies`, `fanout` | dropped | meridian-only |

## MCP tool-policy references

MCP access is gated with inline `mcp(...)` entries in `tools:` (grant) and
`disallowed-tools:` (deny). This replaces the removed `mcp-tools:` frontmatter field.
Grammar and authoring examples: [agent-profiles.md](agent-profiles.md#tools).

Server and tool segments are preserved **verbatim** — no case normalization during parse
or projection. `*` is the only wildcard. Five canonical forms:

| Canonical | Meaning |
|---|---|
| `mcp(server)` | Whole server (equivalent to `mcp(server/*)`) |
| `mcp(server/tool)` | One specific tool |
| `mcp(server/*)` | All tools on one server |
| `mcp(*/tool)` | A tool name across any server |
| `mcp(*/*)` | All MCP tools |

### Per-harness projection

Mars projects canonical `mcp(...)` refs to harness-native permission tokens during native
lowering (Claude agents/skills) and in the **launch bundle** (`ToolsSpec.mcp` carries the
projected tokens for every harness). Unsupported refs are **omitted** from native output
(never broaden permissions) and recorded as approximate lossiness.

| Canonical example | Claude | Cursor | OpenCode | Codex | Pi |
|---|---|---|---|---|---|
| `mcp(server/tool)` | `mcp__server__tool` | `Mcp(server:tool)` | `server_tool` | — | — |
| `mcp(server)` / `mcp(server/*)` | `mcp__server__*` | `Mcp(server:*)` | `server_*` | — | — |
| `mcp(*/tool)` | — (lossy) | `Mcp(*:tool)` | `*_tool` | — | — |
| `mcp(*/*)` | `mcp__*` | `Mcp(*:*)` | `*_*` | — | — |

**Codex** — per-tool MCP gating lives in server config (`mcp_servers.enabled_tools`), not
the agent tool list. Tool-list `mcp(...)` refs record lossiness; whole-server enable is
governed by the `mcp/<name>/mcp.toml` server lane (see
[mcp-and-hooks.md](mcp-and-hooks.md#tool-policy-mcp-references-vs-server-definitions)).

**Pi** — no MCP tool surface; all `mcp(...)` refs are dropped with lossiness.

**Emission surfaces:**

| Surface | MCP behavior |
|---|---|
| Claude agent native | Projected `mcp__…` tokens merged into `tools:` / `disallowed-tools:` (no `mcp-tools:` field) |
| Claude skill native | Allowed MCP projected into `allowed-tools:` (grant semantics; approximate lossiness) |
| Codex / OpenCode / Cursor / Pi agent native | Tool lists not emitted today → MCP lossiness |
| Launch bundle | `ToolsSpec.mcp` = per-harness projected tokens for all harnesses |

Implementation: [`src/compiler/mcp_ref.rs`](../../src/compiler/mcp_ref.rs) (`parse_mcp_ref`,
`project_mcp_ref`). Inbound lift: [`src/staging/.context/CONTEXT.md`](../../src/staging/.context/CONTEXT.md).

## Lossiness Model

Every field lowering is classified as one of four categories:

| Classification | Meaning |
|---|---|
| **exact** | 1:1 native equivalent with identical semantics |
| **approximate** | Closest native equivalent; semantics differ slightly |
| **dropped** | No native equivalent; value is discarded in the native artifact (preserved in `.mars/`) |
| **meridian-only** | Consumed exclusively by Meridian; never lowered to any harness-native format |

Mars emits diagnostics for dropped and approximate fields during `mars sync`:

```
warning[agent-field-dropped]: agent `coder`: field `sandbox` dropped in Claude native artifact
warning[agent-field-approximate]: agent `reviewer`: field `mode` approximately mapped in OpenCode (OpenCode uses the same mode concept)
```

These are non-fatal warnings. The sync continues and the native artifact is still written.

Under `mars validate --strict`, dropped fields with non-default values promote to errors. This lets CI catch cases like `tools: [bash, write]` targeting Codex, which cannot honor the allowlist.

## Lossiness Matrix

Compact per-field, per-target classification:

| Field | `.mars/` | Claude | Codex | OpenCode | Cursor | Pi |
|---|---|---|---|---|---|---|
| `name` | preserved | exact | exact | exact | exact | exact |
| `description` | preserved | exact | exact | exact | exact (one-line normalized) | exact |
| `model` | preserved | exact | exact | exact | exact (internal deterministic adaptation when available) | exact |
| `harness` | preserved | dropped | dropped | dropped | dropped | dropped |
| `mode` | preserved | dropped | dropped | approximate | approximate | approximate |
| `approval` | preserved | dropped | exact | dropped | approximate | dropped |
| `sandbox` | preserved | dropped | exact | dropped | approximate | dropped |
| `tools` | preserved | exact (incl. projected MCP) | dropped | dropped | dropped | dropped |
| `disallowed-tools` | preserved | exact (incl. projected MCP) | dropped | dropped | dropped | dropped |
| `mcp(...)` in `tools` / `disallowed-tools` | preserved | exact (projected into tool lists) | approximate | approximate | approximate | approximate |
| `effort` | preserved | exact | exact | approximate | approximate | approximate |
| `autocompact` | preserved | meridian-only | meridian-only | meridian-only | meridian-only | meridian-only |
| `autocompact_pct` | preserved | meridian-only | meridian-only | meridian-only | meridian-only | meridian-only |
| `skills` | preserved | exact | dropped | dropped | exact | dropped |
| matched `harness-overrides.<target>` passthrough | preserved | meridian-only | meridian-only | meridian-only | meridian-only | meridian-only |
| `model-policies` | preserved | meridian-only | meridian-only | meridian-only | meridian-only | meridian-only |
| `harness-overrides` | preserved | passthrough | passthrough | passthrough | passthrough | passthrough |
| `fanout` | preserved | meridian-only | meridian-only | meridian-only | meridian-only | meridian-only |
| body | preserved | exact | approximate | exact | exact | exact |

## Stale Artifact Cleanup

When an agent is removed from a source package and mars removes it from `.mars/agents/`, mars also removes the corresponding native artifacts from all harness directories (`.claude/agents/`, `.codex/agents/`, etc.) for both `.md` and `.toml` filename shapes.

Cleanup is non-fatal: errors on individual native files are emitted as diagnostics and don't block the sync.

## Dry Run

`mars sync --diff` computes compilation diagnostics (lossiness warnings, validation errors) without writing native artifacts. Use this to preview what would be emitted before running a full sync.
