# Agent Profile Reference

An agent profile is a markdown file with a YAML frontmatter block. The frontmatter declares identity, routing, and runtime policy fields. The markdown body becomes the agent's system prompt (instructions).

```yaml
---
name: coder
description: Implementation agent for code changes
model: gpt55
harness: claude
mode: subagent
approval: auto
effort: high
autocompact: 50
skills: [dev-principles, shared-workspace]
tools: [Bash, Write, Edit, Read]
disallowed-tools: [Agent, Bash(git revert:*)]
harness-overrides:
  codex:
    effort: medium
    sandbox: workspace-write
model-policies:
  - match:
      alias: opus
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
tools: [Bash, Write, Edit]
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
| Allowed values | `claude`, `codex`, `opencode`, `pi` |
| Default | none (universal agent) |

Execution target. When set, mars compiles a harness-native artifact in addition to the canonical `.mars/` artifact. Universal agents (no `harness:`) are installed to `.mars/agents/` only and launched by Meridian against any harness.

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

Context window compaction threshold. Consumed by Meridian's session manager — no harness-native equivalent. Specifies the context percentage at which Meridian triggers compaction.

```yaml
autocompact: 50
```

---

### `skills`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty |

Skill names this agent loads. Skill files are read from the `skills/` directory in the agent's resolved package tree. Mars installs transitive skill dependencies automatically during sync.

```yaml
skills: [dev-principles, shared-workspace]
```

---

### `tools`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty (harness default tool set) |

Tool allowlist. Only these tools are available to the agent. Supports scoped patterns for fine-grained control. Native support varies by harness — see [agent-compilation.md](agent-compilation.md).

```yaml
tools: [Bash, Write, Edit]
tools: [Bash(git status), Write, Read]   # scoped pattern
```

---

### `disallowed-tools`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty |

Tool denylist. These tools are blocked even if they'd otherwise be available. Supports scoped patterns. Native support varies by harness.

```yaml
disallowed-tools: [Agent]
disallowed-tools: [Bash(git revert:*), Bash(git reset:*)]
```

---

### `mcp-tools`

| | |
|---|---|
| Type | string[] |
| Required | no |
| Default | empty |

MCP server references for this agent. Entries are config file references for Claude (`--mcp-config`). Codex uses a different format (`-c mcp.servers.<name>.command`). Behavior varies by harness.

```yaml
mcp-tools: [context7, memory-bank]
```

---

### `harness-overrides`

Per-harness override table. Overrides top-level field values when a specific harness compiles the agent. Only the fields relevant to the target harness are applied; the rest are ignored.

**Overridable fields:** `effort`, `autocompact`, `approval`, `sandbox`, `skills`, `tools`, `disallowed-tools`, `mcp-tools`

**Non-overridable fields (warning if present; field is skipped):** `name`, `description`, `model`, `harness`, `mode`, `harness-overrides`

```yaml
harness-overrides:
  claude:
    approval: auto
    skills: [dev-principles, shared-workspace]
  codex:
    effort: high
    sandbox: workspace-write
```

Override semantics are **replace**: if a field is set in the override block, it fully replaces the top-level value. If it's absent from the override block, the top-level value is used.

At compile time, the matching override block is merged into the lowered artifact. At runtime, Meridian applies the full override table when launching the agent.

---

### `model-policies`

Runtime routing rules consumed by Meridian. Each entry specifies a `match` condition and an `override` to apply when the condition is true.

```yaml
model-policies:
  - match:
      alias: opus
    override:
      effort: high
  - match:
      model: gpt-5.5
    override:
      harness: codex
      effort: medium
```

`model-policies` is Meridian-only — it is preserved in the `.mars/` artifact but stripped from all harness-native compiled outputs.

Mars currently preserves entries as opaque metadata. The `match`/`override` structure above is the Meridian-consumed schema, but mars only validates that `model-policies` is a sequence; it does not validate each entry's internal shape.

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
| Invalid field value (e.g. `effort: ultra`) | Error — field is skipped |
| Unknown harness name | Warning — field is skipped |
| Non-overridable field in override block | Warning — field is skipped |
| Legacy `models:` field | Warning — deprecated; use `fanout:` for display/inventory candidates and `model-policies:` for per-model overrides |
| Unknown top-level fields | Tolerated (forward compatibility) |

Diagnostics are emitted during `mars sync` and `mars validate`. Errors in a field skip that field; the rest of the profile is used.

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

An agent without `harness:` is installed to `.mars/agents/` only. Meridian selects the harness at spawn time based on model resolution and project config.

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
