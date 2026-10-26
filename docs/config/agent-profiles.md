# Agent Profile Reference

An agent profile is a markdown file with a YAML frontmatter block. The frontmatter declares identity, routing and runtime policy. The markdown body becomes the agent's system prompt.

This page describes each field. The format's contract (what schema 1 promises, the removed keys, the value tables) is [Schema v1](schema-v1.md). A profile that predates it can be upgraded with [`mars migrate`](migrating-to-schema-v1.md).

```yaml
---
name: coder
description: Implementation agent for code changes
model: opus
harness: claude
mode: subagent
approval: auto
sandbox: workspace-write
effort: high
skills:
  load: [dev-principles, shared-workspace]
  available: [diagnose]
tools:
  bash: allow
  write: allow
  edit: allow
  read: allow
  agent: deny
  "bash(git revert *)": deny
harness-overrides:
  codex:
    sandbox_workspace_write:
      network_access: true
model-policies:
  - match:
      alias: opus
    no-fallback: true
    override:
      effort: high
    when: Default choice for implementation
---

# Coder

You turn approved blueprints into working code. Focus on correctness, then clarity.
Commit after each passing step.
```

## File Location

| Context | Path |
|---|---|
| Source package | `agents/<name>.md` |
| Project-local | `.mars-src/agents/<name>.md` |

The filename stem is the agent's canonical name — this is what `mars list`, `mars why`, and harness-native directories use to identify the agent.

## File Format

```
---
name: coder
description: Implementation agent for code changes
model: gpt55
harness: claude
skills: [dev-principles, shared-workspace]
tools:
  bash: allow
  write: allow
  edit: allow
---

# Coder

You turn approved plans into working code. Focus on correctness, then clarity.
```

The frontmatter is the block between a first line that is exactly `---` and the next line that is exactly `---`. A file whose first line is something else has no frontmatter: it is all body, and no profile field is set. A fence that opens and never closes is an error. The other file rules are in [Schema v1](schema-v1.md#file-rules).

---

## Fields

### `name`

| | |
|---|---|
| Type | string |
| Required | no |
| Default | none (filename stem is used as fallback) |

Human-readable display name. Appears in `mars list` output and is preserved in compiled artifacts. If omitted, the filename stem serves as the agent's identity everywhere.

```yaml
name: coder
```

---

### `description`

| | |
|---|---|
| Type | string |
| Required | no |
| Default | none |

One-line summary of the agent's purpose. Shown in `mars list` and passed through to harness-native artifacts.

```yaml
description: Implementation agent for code changes
```

---

### `model`

| | |
|---|---|
| Type | string |
| Required | no |
| Default | none (harness default) |

Model alias or concrete model ID. Aliases defined in `[models]` in `mars.toml` are resolved per-target during compilation. If omitted, the harness uses its default model.

```yaml
model: gpt55                # an alias
# model: claude-opus-4-6    # or a concrete model ID
```

---

### `harness`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `claude`, `codex`, `opencode`, `cursor`, `pi` (case-insensitive) |
| Default | none (universal agent) |

Routing and model-selection hint. Native emission is controlled separately by
the agent emission policy. In standalone/default `EmitAll`, every canonical
agent is lowered to every configured target harness, whether or not
`harness:` is set; a nonmatching model field is omitted from that native artifact.
Meridian-managed `SuppressAll` keeps agents canonical-only, while
`EmitSelective` emits only qualifying copies. An unknown harness name is an error.

```yaml
harness: claude
```

See [agent-compilation.md](agent-compilation.md) for what each harness target produces.

---

### `mode`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `primary`, `subagent` |
| Default | none |

Execution mode hint. `primary` agents are launched interactively (full TUI, session-level approval). `subagent` agents are spawned programmatically by orchestrators. Some harnesses use this to select default approval behavior.

```yaml
mode: subagent
```

---

### `model-invocable` and `user-invocable`

| | |
|---|---|
| Type | boolean |
| Required | no |
| Default | `true` |

Who can invoke the agent. `model-invocable: false` keeps another model from choosing the agent by itself; `user-invocable: false` keeps the user from invoking it directly (a slash command or picker). Only the hyphenated spellings exist: `model_invocable` and `user_invocable` are errors that name them. A value must be `true` or `false`; `yes` and `no` are errors.

No native agent file carries either key: an explicit `false` is reported as dropped lossiness. The consumer's `[agents.<name>]` overlay can set both; see [mars.toml](mars-toml.md#agentsname-overlay).

```yaml
model-invocable: false
```

---

### `approval`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `default`, `auto`, `confirm`, `never` |
| Default | none (unset; harness decides) |

Permission approval policy for tool calls.

| Value | Behavior |
|---|---|
| `default` | Explicitly requests harness-default approval behavior |
| `auto` | Auto-approve safe operations (edits, reads); prompt for destructive ops |
| `confirm` | Prompt before every tool call |
| `never` | Never prompt: tool calls run without approval |

Omitting `approval` leaves it unset. `default` is an accepted explicit value, but it is not the same as an omitted field in the parsed profile.

`yolo`, the earlier spelling of `never`, was removed: it is an error that names `never`, and `mars migrate` rewrites it.

```yaml
approval: auto
```

---

### `sandbox`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `default`, `read-only`, `workspace-write`, `danger-full-access` |
| Default | none (unset; harness decides) |

Filesystem access restriction level. Primarily meaningful for Codex. Other harnesses may drop this field — see [agent-compilation.md](agent-compilation.md).

| Value | Behavior |
|---|---|
| `default` | Explicitly requests harness-default sandbox behavior |
| `read-only` | Read filesystem; no writes |
| `workspace-write` | Write within the project workspace |
| `danger-full-access` | Full filesystem access |

Omitting `sandbox` leaves it unset. `default` is an accepted explicit value, but it is not the same as an omitted field in the parsed profile.

```yaml
sandbox: workspace-write
```

---

### `effort`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `none`, `low`, `medium`, `high`, `xhigh`, `max` (case-insensitive) |
| Default | none (the model's default) |

Reasoning effort. `none` means reasoning off and is different from leaving the key out. Mars lowers it to each harness's own control (`--effort` for Claude, `model_reasoning_effort` for Codex); Claude has no off value, so its native file omits `effort` and Mars warns. The per-harness table is in [Schema v1](schema-v1.md#effort).

```yaml
effort: high
```

---

### `skills`

| | |
|---|---|
| Type | string[] or `{ load: string[], available: string[] }` |
| Required | no |
| Default | empty |

Skills this agent gets. A flat list means every skill is loaded. The mapping form separates:

- `load`: the [launch bundle](../cli/commands.md#mars-build-launch-bundle) carries the skill's full body in `skills.loaded`, and Meridian composes it into the agent's system prompt.
- `available`: the bundle lists the skill in `skills.available` (name, type, description).
  Meridian includes the name and description in its composed available-skill inventory;
  the harness's own skill mechanism performs the eventual load.

A name in both lists counts as loaded. `--skill` on `mars build launch-bundle` adds to `load`. Neither list is filtered by the skill's `model-invocable`: a non-invocable skill under `load` still arrives in full, which is how one agent gets a skill no model finds by itself. See [`model-invocable`](skill-compilation.md#model-invocable).

Skill files are read from the `skills/` directory in the agent's resolved package tree. Mars installs transitive skill dependencies automatically during sync.

```yaml
skills: [dev-principles, shared-workspace]
```

```yaml
skills:
  load: [dev-principles, solo-dev]
  available: [diagnose]
```

Harness-native agent compilation preserves this split. Claude and Cursor put only `load` in
their native `skills:` preload field; available-only entries become an end-of-body `/name`
instruction.
Codex, OpenCode, and Pi have no per-agent preload field, so both channels become body guidance
without inlining skill text. Claude also repeats `load` as guidance because `claude --agent`
does not honor the subagent-only `skills:` preload. Body guidance preserves references but is an
approximation because the harness might not act on prose; as the designed target representation,
it does not produce a lossiness warning. See
[Agent Compilation](agent-compilation.md#skill-references-in-native-agents).

---

### `subagents`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty |

The agents this agent is allowed to spawn. `subagents` is a first-class parsed field: mars preserves it in the `.mars/` canonical artifact and uses it during prompt assembly to filter the spawnable-agent **inventory** injected into the system prompt — only listed agents are advertised as spawn options. Meridian reads the same list at spawn time to scope the agent's spawnable roster.

Like `model-policies`, `subagents` is Meridian-oriented metadata — preserved in `.mars/` for runtime consumers, with no harness-native field equivalent.

```yaml
subagents: [explorer, reviewer, coder]
```

---

### `tools`

| | |
|---|---|
| Type | mapping (`tool-name: allow\|deny\|ask`) |
| Required | no |
| Default | empty (harness default tool set) |

Tool policy for the agent, written as rules: a capability, an optional scope, and an action
(`allow`, `deny` or `ask`). Capability names are Mars's snake_case vocabulary
([the capability table](schema-v1.md#tool-capabilities)); a scope goes in parentheses,
`tool(pattern)`. Mars maps each capability to the harness's real tool names while it builds the
launch bundle and lowers agent files; the author never writes a harness spelling.

`tools` is a map. A rule states a grant or a denial and leaves everything else to the harness.
Add `'*': deny` to make the map an allowlist: only the capabilities it allows remain. A
capability that has only scoped allows (`bash(rg *)`) then runs only those commands.

A list (`tools: [read, grep]`), the scalar spellings (`tools: allow`, `tools: Read, Grep`) and
`disallowed-tools` are errors in schema 1. A denial is `tools: {agent: deny}`.
`tools: {replace: [...]}` is an error as well: replacement tables belong to the consumer's
`[agents.<name>.tools]` overlay, and a profile has no lower package policy to replace. See
[Adding to or replacing a permission list](mars-toml.md#adding-to-or-replacing-a-permission-list).

`ask` asks before the call. Native agent files (`.claude/agents/`) have no field for it, or
for an allowlist with nothing allowed; Mars reports both as dropped lossiness. Meridian
enforces them from the launch bundle.

A scope written with the legacy prefix suffix, `bash(git revert:*)`, is normalized to
`bash(git revert *)`, the one spelling every harness reads.

```yaml
tools:                                   # grants and denials
  bash: allow
  "bash(meridian spawn *)": allow
  "bash(git push *)": ask
  agent: deny
  edit: deny
  "bash(git status *)": allow           # scoped pattern
  mcp(context7): allow                   # MCP grants — see below
  mcp(plugin:demo/query-docs): allow
  ls: allow
```

```yaml
tools:                                   # explicit allowlist
  "*": deny
  "bash(rg *)": allow
```

Readable aliases and native spellings (`Bash`, `shell`, `exec_command`, `AskUserQuestion`,
`websearch`) resolve to the same capability. A name Mars does not recognize is kept and
reported as `unknown` in the launch bundle; Mars never guesses a native spelling for it.
`ls` and `work` are capabilities too: `ls` lists one folder (OpenCode, Pi and Meridian Flow have
a tool for it), and `work` is Meridian Flow's work tool.

**MCP grants** — inline `mcp(...)` entries in `tools` gate MCP server/tool access. Server and tool segments are preserved **verbatim** (case-sensitive). `*` is the only wildcard.

| Form | Meaning |
|---|---|
| `mcp(server)` | Whole server (shorthand for `mcp(server/*)`) |
| `mcp(server/tool)` | One specific tool |
| `mcp(server/*)` | All tools on one server |
| `mcp(*/tool)` | A tool name across any server |
| `mcp(*/*)` | All MCP tools |

```yaml
tools:
  mcp(context7): allow
  mcp(memory-bank/store): allow
  mcp(github/delete_repo): deny
```

The earlier `mcp-tools:` / `mcp_tools` frontmatter field was **removed**: write the same
grants as `mcp(server): allow` entries in `tools`. Per-harness projection and lossiness are
documented in [agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).
These entries govern tool permissions, not server registration. Server definitions come
from package `mcp/<name>/mcp.toml` files and are installed by `mars sync`.

---

### `harness-overrides`

Per-harness target-native passthrough table. Mars writes each harness's block into that harness's native file, and passes the block matching the resolved harness to the launch bundle's `execution_policy.native_config`. Nested keys are target-native: Mars validates only that the block is a mapping of serializable values.

Top-level Mars fields remain the semantic controls for routing, prompt assembly, tool policy, skills, and execution policy. A key inside `harness-overrides.<harness>` does not replace a field Mars lowers itself; it is dropped with a warning.

```yaml
harness-overrides:
  claude:
    color: red                # written to .claude/agents/<name>.md only
  codex:
    sandbox_workspace_write:  # Codex config keys, carried as written
      network_access: true
  flow:
    permission: read          # Meridian Flow's own keys; reserved namespace
```

The block also reaches the launch bundle as `execution_policy.native_config`; the launcher decides how to apply it. For Meridian, Codex applies it as `-c` overrides.

A Claude plugin's Claude-only keys (`color`, `maxTurns`, `permissionMode`, `memory`, `background`, `isolation`, `initialPrompt`, `hooks`) arrive here: importing the plugin moves them, unchanged and in file order, to `harness-overrides.claude`, so `.claude/` keeps them and no other harness sees them.

Use top-level fields for Mars semantics. Use `harness-overrides` for harness-specific config Mars should carry without interpreting. `flow` is a reserved consumer namespace, not a harness: Mars keeps it and never interprets it, and routing never selects it. Any other unknown name is a warning, and its block is kept. See [Schema v1](schema-v1.md#harness-overrides-and-the-flow-namespace).

---

### `model-policies`

Conditional model settings, declared backups and a per-model guide for this agent. Each rule has a `match` (exactly one of `alias`, `model`, `model-glob`), an optional `override` of `effort`, `sandbox` and `approval`, an optional `when`, and an optional `no-fallback`.

```yaml
model-policies:
  - match:
      alias: opus
    override:
      effort: high
      sandbox: workspace-write
    when: Architecture review that crosses module boundaries
  - match:
      model: gpt-5.5
    override:
      effort: medium
  - match:
      model-glob: "gpt-5*"
    no-fallback: true
```

Mars takes the first rule that matches the chosen model; only that rule's `override` applies, and it outranks the agent's top-level `effort`, `sandbox` and `approval`. The full precedence, the consumer overlay and settings layers, and `when` are in [Schema v1](schema-v1.md#model-policies). Any `override` key but `effort`, `sandbox` and `approval` is an error.

**Declared backups.** For an implicit model choice, launch fallback scans **all concrete profile rules**
in declaration order after trying the primary. It does not depend on where the
primary's settings matched. `no-fallback: true` excludes only that entry from
fallback; it cannot stop later entries or prevent using the model as primary or
explicitly. Flagged entries still supply settings when they match the chosen model.
Explicit model requests do not use model backups.

Fallback skips globs and deduplicates by selector kind/token. Distinct aliases are
not merged because they resolve to the same model. Overlay/global rules provide
settings, not additional profile fallback candidates. Empty overrides stay valid, and a
rule with no `override` is a candidate-only entry.

Inventory labels these rules as declared backups, with `(model ID)` marking literal
selectors. They describe automatic fallback, not equivalent explicit overrides:
`--model TOKEN` resolves an alias named `TOKEN` before treating it as a model ID.

**Guide.** `mars models prompting @agent` lists each rule that has a `when`, in declaration order, ahead of the model alias's own `prompting` text.

The rule list is preserved in `.mars/` but not emitted as a harness-native field.
Native agent materialization applies `[settings.meridian.agent_copy] include_fanout`: with
it, flagged rules and expanded globs can also qualify an agent for a harness. Launch fallback
filtering does not change native emission or reconciliation.

Mars parses the `match`/`override` structure when it reads the profile and reports a malformed
shape as an error; `mars check` also warns when a rule's `alias` is not a defined alias.

---

## Validation

Mars reads every profile with the schema reader and reports diagnostics, each with a code and the key path:

| Condition | Severity |
|---|---|
| A removed key (`network`, `disallowed-tools`, `fanout`, ...), a list-form `tools`, an invalid value (`effort: ultra`, `approval: yolo`), malformed `model-policies`, an unclosed or non-mapping frontmatter, a duplicate key | Error |
| An unknown harness name in `harness:` | Error |
| An unknown top-level field | Warning — the field stays in the file but reaches no native file; put harness-specific keys under `harness-overrides.<harness>` |
| An unknown name in `harness-overrides` (other than `flow`) | Warning — the block is preserved |

Errors fail `mars sync`, `mars check` and `mars validate` for the package that carries them, before anything is written. `mars check` exits 1 when it finds errors; `mars sync` exits 2. A profile of a package without `schema = 1` is upgraded in memory first, with one warning that names `mars migrate`; see [Migrating to schema v1](migrating-to-schema-v1.md).

---

## Examples

### Minimal profile

```yaml
---
name: planner
---

# Planner

You produce structured plans from requirements.
```

### Universal agent (no harness)

An agent without `harness:` remains routable to any harness. It is always
installed to `.mars/agents/`; native copies depend on emission policy. The
standalone default emits it to every configured target harness, while
Meridian-managed suppression keeps it canonical-only. Meridian selects the
harness at spawn time based on model resolution and project config.

```yaml
---
name: reviewer
description: Adversarial review of designs and code changes
model: gpt-5.4
mode: subagent
skills: [review-principles]
---

# Reviewer

You review designs and code for correctness, regressions, and structural soundness.
Report findings with severity. Read-only — do not edit.
```
