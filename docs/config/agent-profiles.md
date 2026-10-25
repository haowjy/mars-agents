# Agent Profile Reference

An agent profile is a markdown file with a YAML frontmatter block. The frontmatter declares identity, routing, and runtime policy fields. The markdown body becomes the agent's system prompt (instructions).

```yaml
---
name: coder
description: Implementation agent for code changes
model: opus
harness: claude
mode: subagent
approval: auto
sandbox: workspace-write
network: allow
effort: high
autocompact: 200000
autocompact_pct: 80
skills: [dev-principles, shared-workspace]
tools:
  bash: allow
  write: allow
  edit: allow
  read: allow
disallowed-tools: [agent, bash(git revert:*)]
harness-overrides:
  codex:
    effort: medium
    sandbox: workspace-write
model-policies:
  - match:
      alias: opus
    no-fallback: true
    override:
      effort: high
fanout:
  - alias: opus
  - alias: codex
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

Everything before the first `---` fence, or files with no frontmatter, is treated as a body-only agent with no profile fields set.

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
model: gpt55        # alias
model: claude-opus-4-6  # concrete ID
```

---

### `harness`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `claude`, `codex`, `opencode`, `cursor`, `pi` |
| Default | none (universal agent) |

Routing and model-selection hint. Native emission is controlled separately by
the agent emission policy. In standalone/default `EmitAll`, every canonical
agent is lowered to every configured target harness, whether or not
`harness:` is set; a nonmatching model field is omitted from that native artifact.
Meridian-managed `SuppressAll` keeps agents canonical-only, while
`EmitSelective` emits only qualifying copies.

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

### `approval`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `default`, `auto`, `confirm`, `yolo` |
| Default | none (unset; harness decides) |

Permission approval policy for tool calls.

| Value | Behavior |
|---|---|
| `default` | Explicitly requests harness-default approval behavior |
| `auto` | Auto-approve safe operations (edits, reads); prompt for destructive ops |
| `confirm` | Prompt before every tool call |
| `yolo` | Approve all tool calls without prompting |

Omitting `approval` leaves it unset. `default` is an accepted explicit value, but it is not the same as an omitted field in the parsed profile.

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

### `network`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `allow`, `deny`, `allowlist` |
| Default | none (unset; Meridian follows the resolved sandbox) |

Controls whether the agent's shell commands may open connections, including the
public internet, host services, and local sockets. It does not control built-in
web tools; grant or deny those with `tools: web`.

| Value | Behavior |
|---|---|
| `allow` | Allow shell commands to open connections |
| `deny` | Deny shell commands from opening connections |
| `allowlist` | Allow only `network-allow` hosts, minus `network-deny` hosts |

Mars carries an omitted mode as `null` in the launch bundle; it does not choose a network
default unless `[settings] default_network` (project, local, or user config) supplies one.
Meridian resolves a remaining unset mode from the sandbox. There is no explicit `default`
value. Harness projection reports enforcement limits.

At launch, the mode resolves in this order: CLI `--network`, agent overlay,
matching overlay model policy, profile, matching profile model policy, then
matching settings model policy, project/local settings, and user config.

```yaml
network: allowlist
network-allow: [github.com, "*.github.com", "**.pypi.org"]
network-deny: [uploads.example]
```

`network-allow` and `network-deny` are permission sets. Plain lists add across user,
project, local, profile, overlay, and CLI surfaces. `{replace: [...]}` clears lower entries
([how lists combine](mars-toml.md#adding-to-or-replacing-a-permission-list));
a package profile may replace its allow set but may not replace its deny set. Each entry is
an exact host, an IP address, `*.x` (subdomains only), or `**.x` (apex plus subdomains).
Wildcards require at least two DNS labels. Mars lowercases hosts, removes a trailing dot,
canonicalizes IPv4-mapped IPv6 to IPv4, deduplicates, and sorts them. It rejects URLs, schemes,
ports, paths, numeric or `0x`-style terminal DNS labels, empty strings, and a bare `*`;
internationalized names must use punycode. `network_allow` / `network_deny` are accepted
aliases. An unknown network-looking frontmatter key produces a warning instead of being
silently ignored. An empty `allowlist` fails closed to `deny` with a warning.

---

### `effort`

| | |
|---|---|
| Type | string |
| Required | no |
| Allowed values | `low`, `medium`, `high`, `xhigh`, `max` |
| Default | none (harness model default) |

Reasoning effort level. Maps to model-specific effort controls (`--effort` for Claude, `model_reasoning_effort` for Codex). `max` is accepted as an alias for `xhigh`; Claude lowering normalizes `xhigh` to `max`.

```yaml
effort: high
```

---

### `autocompact`

| | |
|---|---|
| Type | integer |
| Required | no |
| Range | 0–4294967295 |
| Default | none |

Context window compaction threshold in tokens. It is preserved in the launch bundle rather
than native agent files. Meridian maps it to Claude's token-window flag and Codex's token
limit; harnesses without a matching control report it unsupported.

```yaml
autocompact: 200000
```

---

### `autocompact_pct`

| | |
|---|---|
| Type | integer |
| Required | no |
| Range | 1–100 |
| Default | none |

Context window compaction threshold as a percentage of the context window. It is preserved
in the launch bundle rather than native agent files. Meridian maps it to Claude's percentage
environment variable and Codex's percentage config; harnesses without a matching control
report it unsupported.

```yaml
autocompact_pct: 80
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

Like `model-policies` and `fanout`, `subagents` is Meridian-oriented metadata — preserved in `.mars/` for runtime consumers, with no harness-native field equivalent.

```yaml
subagents: [explorer, reviewer, coder]
```

---

### `tools`

| | |
|---|---|
| Type | string[] or mapping (`tool-name: allow\|deny\|ask`) |
| Required | no |
| Default | empty (harness default tool set) |

Tool policy for the agent, written as rules: a capability, an optional scope, and an action
(`allow`, `deny` or `ask`). Capability names are Mars's snake_case vocabulary (below); a
scope goes in parentheses, `tool(pattern)`. Mars maps each capability to the harness's real
tool names while it builds the launch bundle and lowers agent files; the author never writes
a harness spelling.

Two forms:

- **List form is deprecated.** `tools: [read, grep]` still only grants those entries and
  restricts nothing. A non-empty list emits a deprecation warning. An empty list names no tools
  and stays "no policy".
- **Map form states rules.** `tools: {read: allow, edit: deny}` grants and denies those
  entries and leaves everything else to the harness. Add `'*': deny` to make a map an
  allowlist too. A capability that has only scoped allows (`bash(rg *)`) then runs only those
  commands.

`tools: {replace: [...]}` is invalid in a package profile. Replacement tables belong to
consumer `[agents.<name>.tools]` and `[skills.<name>.tools]` overlays; a profile has no lower
package policy to replace. See [Adding to or replacing a permission list](mars-toml.md#adding-to-or-replacing-a-permission-list).

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
tools:                                   # explicit allowlist in map form
  "*": deny
  "bash(rg *)": allow
```

Readable aliases and native spellings (`Bash`, `shell`, `exec_command`, `AskUserQuestion`,
`websearch`) resolve to the same capability. A name Mars does not recognize is kept as
written and reported as `unknown` in the launch bundle; Mars never guesses a native spelling
for it. `tools: allow` / `tools: deny` (a scalar) is a validation error. Any other scalar is
Claude's own comma-separated spelling, `tools: Read, Grep`, and reads as the list
`[Read, Grep]`, granting both entries (commas inside a scope, `bash(rg a,b)`, stay with their
entry).

**Capabilities.** One name per kind of tool. Where a capability covers several native tools,
a rule covers all of them.

| Capability | Meaning |
|---|---|
| `bash` | Shell commands |
| `read`, `write`, `edit` | Read files; create files; modify files (`edit` covers writes and notebook edits on harnesses where they are separate tools) |
| `notebook` | Jupyter notebook editing |
| `glob`, `grep` | File and content search |
| `agent` | Sub-agents (Claude `Agent` with `SendMessage` and `ListAgents`, OpenCode `task`) |
| `task` | The task list (Claude `TaskCreate`…`TaskUpdate`, OpenCode `todowrite`, Codex `update_plan`). Not sub-agents. Names are case-insensitive here, so `Task` is the task list in a `dialect = "mars-native"` package; in any other package staging reads `Task` as Claude's old sub-agent tool and lifts it to `agent` |
| `skill`, `workflow` | Loading a skill; running a workflow |
| `web` | `web_search` and `web_fetch` together |
| `web_search`, `web_fetch` | One of the two |
| `ask_user` | Asking the user a question |
| `plan_mode`, `worktree`, `lsp`, `cron`, `monitor`, `notifications`, `send_user_file`, `schedule_wakeup`, `remote_trigger`, `tool_search` | Harness features that exist on some harnesses only |

Per-harness native names and the loss rules are in
[agent-compilation.md](agent-compilation.md#tool-name-projection).

**MCP grants** — inline `mcp(...)` entries in `tools:` or `disallowed-tools:` gate MCP
server/tool access. Server and tool segments are preserved **verbatim** (case-sensitive).
`*` is the only wildcard.

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
disallowed-tools: [mcp(github/delete_repo)]
```

The legacy `mcp-tools:` / `mcp_tools` frontmatter field is **removed** — use
`tools: {mcp(server): allow}` instead. Per-harness projection and lossiness are documented in
[agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).
These entries govern tool permissions, not server registration. Server definitions come
from package `mcp/<name>/mcp.toml` files and are installed by `mars sync`.

---

### `disallowed-tools`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty |

Tool denylist. These tools are blocked even if they'd otherwise be available. Uses the same canonical tool-name grammar and scoped pattern syntax as `tools`.

```yaml
disallowed-tools: [agent]
disallowed-tools: [bash(git revert:*), bash(git reset:*)]
disallowed-tools: [mcp(plugin:audit/*)]   # MCP denials use the same mcp(...) grammar
```

---

### `harness-overrides`

Per-harness target-native passthrough table. Mars selects the block matching the resolved harness and preserves its keys/values exactly for the launch bundle's `execution_policy.native_config`. Nested keys are target-native: Mars validates only that the block is a mapping of serializable values.

Top-level Mars fields remain the semantic controls for routing, prompt assembly, tool policy, skills, and execution policy. A key inside `harness-overrides.<harness>` does not replace the top-level field with the same name.

```yaml
harness-overrides:
  claude:
    customClaudeKey: true
    tools: [ask_user, askuser]  # preserved exactly in passthrough
  codex:
    sandbox_workspace_write:  # Codex config keys, carried as written
      network_access: true
```

The selected block reaches the launch bundle as `execution_policy.native_config`; the launcher decides how to apply it. For Meridian, Codex applies it as `-c` overrides.

Use top-level fields for Mars semantics. Use `harness-overrides` for harness-specific config Mars should carry without interpreting.

Nested values are serializability-validated only. Mars preserves entries as target-harness native passthrough and does not interpret key semantics.

---

### `model-policies`

Conditional model settings and profile fallback candidates resolved by Mars.
`match` selects an `alias`, a literal `model` ID, or a `model-glob`. Optional
`override` settings apply to the first matching rule in overlay → profile → settings
order. Omit `override` or use `{}` for a candidate-only entry.

For an implicit model choice, launch fallback scans **all concrete profile rules**
in declaration order after trying the primary. It does not depend on where the
primary's settings matched. `no-fallback: true` excludes only that entry from
fallback; it cannot stop later entries or prevent using the model as primary or
explicitly. Flagged entries still supply settings when they match the chosen model.
Explicit model requests do not use model backups.

Fallback skips globs and deduplicates by selector kind/token. Distinct aliases are
not merged because they resolve to the same model. Overlay/global rules provide
settings, not additional profile fallback candidates. Empty overrides stay valid.

Inventory labels these rules as declared backups, with `(model ID)` marking literal
selectors. They describe automatic fallback, not equivalent explicit overrides:
`--model TOKEN` resolves an alias named `TOKEN` before treating it as a model ID.

```yaml
model-policies:
  - match:
      alias: opus
    override:
      effort: high
      sandbox: workspace-write
      network: deny
  - match:
      model: gpt-5.5
    override:
      harness: codex
      effort: medium
```

The rule list is preserved in `.mars/` but not emitted as a harness-native field.
Native agent materialization has separate `include_fanout` semantics: flagged rules
and expanded globs can still participate. Launch fallback filtering does not change
native emission or reconciliation.

Mars parses the `match`/`override` structure during `mars check` and before
`mars version`, reporting malformed shape. `override` may be omitted or empty.
Launch-bundle execution overrides include `effort`, `approval`, `sandbox`,
`network`, `autocompact`, and `autocompact_pct`; `network` uses the same policy
layer and precedence as `sandbox`.

---

### `fanout`

Declares additional model candidates for inventory display in `meridian spawn --list`. Entries describe fallback or alternative models the agent can run on.

```yaml
fanout:
  - alias: opus
  - alias: codex
  - model: gpt-5.5
```

`fanout` is Meridian-only — preserved in `.mars/` but stripped from harness-native artifacts.

Mars currently preserves fanout entries as opaque metadata and only validates that `fanout` is a sequence; it does not validate each entry's internal shape.

---

## Validation

Mars validates agent profiles at compile time and emits diagnostics:

| Condition | Severity |
|---|---|
| Invalid field value (e.g. `effort: ultra`, malformed `model-policies`) | Error — field is skipped |
| Unknown harness name | Warning — block is preserved for forward compatibility |
| Legacy `models:` field | Warning — deprecated; use `fanout:` for display/inventory candidates and `model-policies:` for per-model overrides |
| Unknown top-level fields | Tolerated (forward compatibility) |

Diagnostics are emitted during `mars sync` and `mars validate`. `mars check` and `mars version` use the same parser diagnostics as package-gating errors. Errors in a field skip that field during sync/validate; package gates fail until the source package is fixed.

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
