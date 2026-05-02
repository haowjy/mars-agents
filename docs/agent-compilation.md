# Agent Compilation

During `mars sync`, agents are compiled to two surfaces: a canonical full-fidelity artifact in `.mars/agents/`, and — when the agent declares `harness:` — a native artifact in the corresponding harness directory (`.claude/agents/`, `.codex/agents/`, etc.).

## The Two Surfaces

Every agent produces a `.mars/agents/<name>.md` artifact regardless of whether `harness:` is set. This is the canonical compiled output: all frontmatter fields preserved, body unchanged. Meridian reads from here at spawn time.

When `harness:` is set, a second artifact is emitted in the harness-native directory:

| `harness:` value | Native artifact |
|---|---|
| `claude` | `.claude/agents/<name>.md` |
| `codex` | `.codex/agents/<name>.toml` |
| `opencode` | `.opencode/agents/<name>.md` |
| `pi` | `.pi/agents/<name>.md` |

Harness-native artifacts are format-translated and field-stripped. They serve as agent discovery surfaces for harness-native invocation (e.g. `codex --agent coder`). Meridian always uses the `.mars/` artifact for its own spawn logic and applies all policy fields through its own projection layer.

Universal agents (no `harness:`) are installed to `.mars/agents/` only and can be launched by Meridian against any harness.

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

**`MERIDIAN_MANAGED=1`** — when Meridian invokes mars, it sets this env var. Under `auto`, native artifacts are suppressed: Meridian manages harness-native agent loading itself and doesn't want mars-compiled artifacts competing with its runtime projection. Set `agent_emission = "always"` to override this.

When emission is disabled (via `never` or `MERIDIAN_MANAGED=1` with `auto`), mars removes any previously-emitted native artifacts for harness-bound agents currently in `.mars/agents/`.

This cleanup only removes the native artifact for the agent's current `harness:` value. If an agent previously targeted a different harness, the old native artifact may remain stale; run `mars sync` after changing an agent's harness to clean up the previous target.

## Harness-Override Merge

Before lowering to a native artifact, mars merges the `harness-overrides.<target>` fields currently consulted by lowering into the top-level profile values: `effort`, `approval`, `sandbox`, `skills`, `tools`, and `disallowed-tools`.

1. Start with the agent's top-level field values.
2. If `harness-overrides.<target>` exists, overlay those fields (replace semantics — a field present in the override block replaces the top-level value).
3. Lower the merged field values to the target's native format.

Example: an agent has `effort: low` and `harness-overrides.codex.effort: high`. The Codex artifact gets `model_reasoning_effort = "high"`. The `.mars/` artifact preserves both the top-level `effort: low` and the full `harness-overrides:` table for Meridian's runtime use.

`OverrideFields` also contains `mcp-tools` and `autocompact`, but the current lowering merge does not read them. `mcp-tools` lowering reads from the top-level profile field directly.

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
| `tools` | `tools:` | exact |
| `disallowed-tools` | `disallowed-tools:` | exact |
| `mcp-tools` | `mcp-tools:` | exact |
| `effort` | `effort:` (`xhigh` → `max`) | exact |
| `approval` | dropped | dropped |
| `sandbox` | dropped | dropped |
| `mode` | dropped | dropped |
| `autocompact` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `harness-overrides` | merged, then dropped | — |
| `fanout` | dropped | meridian-only |
| `harness` | dropped | dropped |

`approval` and `sandbox` policy fields are applied at launch time by Meridian through its harness projection layer, not stored in the agent file.

### `.codex/agents/<name>.toml`

TOML format. Codex reads this for native agent invocation.

| Source field | Codex native key | Classification |
|---|---|---|
| `name` | `[agent].name` | exact |
| `description` | `[agent].description` | exact |
| `model` | `[agent].model` | exact |
| `effort` | `[agent.config].model_reasoning_effort` | exact |
| `sandbox` | `[agent.config].sandbox_mode` | exact |
| `approval` | `[agent.config].approval_policy` | exact |
| body | `[agent.instructions].content` | approximate (TOML string) |
| `skills` | dropped | dropped |
| `tools` | dropped | dropped |
| `disallowed-tools` | dropped | dropped |
| `mcp-tools` | `-c mcp.servers.<name>.command` | approximate |
| `mode` | dropped | dropped |
| `autocompact` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |

**Approval value mapping:**

| `approval:` | `approval_policy` |
|---|---|
| `default` | (omitted) |
| `auto` | `"on-request"` |
| `confirm` | `"untrusted"` |
| `yolo` | `"bypass"` |

**Example output:**

```toml
[agent]
name = "coder"
description = "Implementation agent for code changes"
model = "gpt55"

[agent.config]
model_reasoning_effort = "high"
sandbox_mode = "workspace-write"
approval_policy = "on-request"

[agent.instructions]
content = """
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
| `mcp-tools` | session payload or error | approximate |
| `autocompact` | dropped | meridian-only |
| `model-policies` | dropped | meridian-only |
| `fanout` | dropped | meridian-only |

`skills` are not emitted into the OpenCode agent artifact. Skill availability comes from separate skill-surface compilation to `.opencode/skills/`.

`effort` on OpenCode subprocess maps to `--variant`; on streaming/TUI launches it's dropped.

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
| All other policy fields | dropped | dropped |
| `autocompact`, `model-policies`, `fanout` | dropped | meridian-only |

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

Under `mars validate --strict`, dropped fields with non-default values promote to errors. This lets CI catch cases like `tools: [Bash, Write]` targeting Codex, which cannot honor the allowlist.

## Lossiness Matrix

Compact per-field, per-target classification:

| Field | `.mars/` | Claude | Codex | OpenCode | Pi |
|---|---|---|---|---|---|
| `name` | preserved | exact | exact | exact | exact |
| `description` | preserved | exact | exact | exact | exact |
| `model` | preserved | exact | exact | exact | exact |
| `harness` | preserved | dropped | dropped | dropped | dropped |
| `mode` | preserved | dropped | dropped | approximate | approximate |
| `approval` | preserved | dropped | exact | dropped | dropped |
| `sandbox` | preserved | dropped | exact | dropped | dropped |
| `tools` | preserved | exact | dropped | dropped | dropped |
| `disallowed-tools` | preserved | exact | dropped | dropped | dropped |
| `mcp-tools` | preserved | exact | approximate | approximate | n/a |
| `effort` | preserved | exact | exact | approximate | approximate |
| `autocompact` | preserved | meridian-only | meridian-only | meridian-only | meridian-only |
| `skills` | preserved | exact | dropped | dropped | dropped |
| `model-policies` | preserved | meridian-only | meridian-only | meridian-only | meridian-only |
| `harness-overrides` | preserved | merged | merged | merged | merged |
| `fanout` | preserved | meridian-only | meridian-only | meridian-only | meridian-only |
| body | preserved | exact | approximate | exact | exact |

## Stale Artifact Cleanup

When an agent is removed from a source package and mars removes it from `.mars/agents/`, mars also removes the corresponding native artifacts from all harness directories (`.claude/agents/`, `.codex/agents/`, etc.) for both `.md` and `.toml` filename shapes.

Cleanup is non-fatal: errors on individual native files are emitted as diagnostics and don't block the sync.

## Dry Run

`mars sync --diff` computes compilation diagnostics (lossiness warnings, validation errors) without writing native artifacts. Use this to preview what would be emitted before running a full sync.
