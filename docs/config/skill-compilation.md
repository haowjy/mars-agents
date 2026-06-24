# Skill Compilation

Skills use a universal frontmatter schema that mars compiles to per-harness native field formats during `mars sync`. This separates skill authoring from harness-specific field names — a skill author writes `model-invocable: false` once, and mars emits the right field for Claude, Codex, or any other target.

## Universal Skill Frontmatter

All skill fields below are optional. A skill with no frontmatter is valid and is treated as body-only with both model and user invocability enabled.

```yaml
---
name: my-skill
description: What this skill does
model-invocable: false
tools: [bash(git *), read, write, mcp(plugin:demo)]
disallowed-tools: [agent]
license: MIT
metadata:
  owner: platform-team
---

# My Skill

Skill instructions...
```

### `name`

| | |
|---|---|
| Type | string |
| Required | yes (when frontmatter present) |

Human-readable display name. Used in `mars list` and preserved in compiled artifacts.

---

### `description`

| | |
|---|---|
| Type | string |
| Required | yes (when frontmatter present) |

One-line summary. Shown in `mars list` and passed through to native artifacts.

---

### `type`

| | |
|---|---|
| Type | string (free-form) |
| Required | no |
| Default | `reference` |

Classifies the skill for prompt-assembly ordering and `mars skills` filtering. `type` is **not** a restricted enum — any string is accepted and preserved.

Mars uses it for exactly two things:

1. **Sort priority during prompt assembly.** Loaded skills are ordered by tier so higher-priority guidance lands first:

   | `type` | Priority |
   |---|---|
   | `principle` | 0 (first) |
   | `guardrail` | 1 |
   | `reference`, any other value, or unset | 2 (last) |

2. **Filtering.** `mars skills list --type <value>` matches `type` by exact string.

A skill with no `type` defaults to `reference`. Custom values (e.g. `type: workflow`) are accepted, sort at priority 2, and stay filterable by their exact string — but they gain no behavior beyond `reference`.

```yaml
type: principle
```

---

### `model-invocable`

| | |
|---|---|
| Type | boolean |
| Default | `true` |

Controls whether the model can see and self-load this skill.

```yaml
model-invocable: false   # model cannot self-invoke
```

---

### `user-invocable`

| | |
|---|---|
| Type | boolean |
| Default | `true` |

Controls whether the user can trigger this skill with `/name`.

```yaml
user-invocable: false   # user cannot trigger via /name
```

**Removed fields.** `invocation`, `disable-model-invocation`, and `allow_implicit_invocation` are no longer recognized. Using them produces a user-visible `skill-schema-error` diagnostic whose message says the field was removed; the parser-level variant is `RemovedField`. Migrate to `model-invocable` / `user-invocable`.

---

### `tools`

| | |
|---|---|
| Type | string[] **or** map of `tool-pattern: allow\|deny` |
| Default | empty |

Tool allowlist and inline denials for this skill — identical schema to agent profiles. Uses Mars canonical semantic snake_case tool names (`bash`, `ask_user`, `web_search`) and supports scoped patterns. Readable aliases and native spellings such as `Bash`, `shell`, and `askuser` are accepted and canonicalized.

List form is allowlist-only:

```yaml
tools: [bash(git *), read]
```

Map form mixes allow and deny entries; denials merge with `disallowed-tools`:

```yaml
tools:
  ask_user: allow
  "bash(git *)": deny
```

Dropped by some harnesses — see the lossiness table below.

**Canonical skills do not use `allowed-tools`.** Authoring `allowed-tools` or
`allowed_tools` directly in a skill's frontmatter produces a `skill-schema-warning`
diagnostic (Validation category, always surfaced regardless of lossiness
settings) and the key is **stripped** before the skill reaches `.mars/`. The
diagnostic is emitted at the staging boundary, before the strip. Use the canonical
`tools:` field instead.

During staging for foreign dialects (Claude, Codex, etc.), harness-native
`allowed-tools` / `allowed_tools` are lifted to canonical `tools:` — this is
the normal import path for foreign-authored skills, distinct from the warning
path above.

---

### `disallowed-tools`

| | |
|---|---|
| Type | string[] |
| Default | empty |

Flat tool denylist merged with map-form `tools:` denials. Lowered to harness-native denylist fields where supported (Claude, Pi); warn-dropped elsewhere.

```yaml
disallowed-tools: [agent, web_search]
```

**MCP grants** — same `mcp(...)` grammar as agent profiles (see
[agent-profiles.md](agent-profiles.md#tools)). Author grants in `tools:` and denials in
`disallowed-tools:`. The legacy `mcp-tools:` field is **removed**.

On Claude, allowed MCP refs project into `allowed-tools:` as harness-native tokens (a
**grant**, not a restriction — Mars records approximate lossiness). Other harnesses drop
or approximate MCP from native skill artifacts; the launch bundle carries the real
per-harness projection. See [agent-compilation.md](agent-compilation.md#mcp-tool-policy-references).

```yaml
tools: [bash(git *), mcp(plugin:context7)]
```

---

### `license`

| | |
|---|---|
| Type | string |
| Default | none |

SPDX license identifier or license text. Preserved in all native artifacts.

```yaml
license: MIT
```

---

### `metadata`

| | |
|---|---|
| Type | YAML mapping |
| Default | none |

Arbitrary key-value metadata. Not interpreted by mars or Meridian; passed through to all native artifacts for use by downstream tooling.

```yaml
metadata:
  owner: platform-team
  tier: core
```

---

## Unknown Fields

Skill frontmatter fields not listed above are **passed through** to all native targets unchanged. Mars preserves unrecognized fields verbatim during lowering — harness-specific fields like Claude Code's `argument-hint` and `arguments` survive compilation and reach the target artifact.

This means you can add harness-native fields directly to your skill frontmatter and they will appear in the compiled output for every target. Mars does not validate or interpret them — it copies the YAML value as-is.

```yaml
---
name: my-skill
description: Does a thing
argument-hint: "What should I review?"
arguments:
  - name: target
    description: File or directory to review
    required: true
---
```

## Per-Harness Lowering

Mars compiles universal frontmatter fields to each target's native field names during `mars sync`. Skill invocability maps to different native fields per harness.

### Field mapping table

| Field | `.mars/` | Claude | Codex | OpenCode | Pi | Cursor |
|---|---|---|---|---|---|---|
| `name` | preserved | `name` | `name` | `name` | `name` | `name` |
| `description` | preserved | `description` | `description` | `description` | `description` | `description` |
| `model-invocable: false` | preserved | `disable-model-invocation: true` | `allow_implicit_invocation: false`¹ | dropped | `disable-model-invocation: true` | `disable-model-invocation: true` |
| `model-invocable: true` | preserved | (omit) | (omit or `allow_implicit_invocation: true`)¹ | (omit) | (omit) | (omit) |
| `user-invocable: false` | preserved | `user-invocable: false` | dropped | dropped | dropped | dropped |
| `user-invocable: true` | preserved | (omit) | (omit) | (omit) | (omit) | (omit) |
| `tools` | preserved | `allowed-tools` (+ projected MCP grants) | dropped | dropped | `allowed-tools` | dropped |
| `disallowed-tools` | preserved | `disallowed-tools` (+ projected MCP denials) | dropped | dropped | `disallowed-tools` | dropped |
| `mcp(...)` in `tools` / `disallowed-tools` | preserved | projected into `allowed-tools` / `disallowed-tools` | approximate | approximate | dropped | approximate |
| `license` | preserved | `license` | `license` | `license` | `license` | `license` |
| `metadata` | preserved | `metadata` | `metadata` | `metadata` | `metadata` | `metadata` |

¹ Codex only emits `allow_implicit_invocation` when the source skill explicitly set `model-invocable`. Skills with no `model-invocable` field do not gain an `allow_implicit_invocation` field in the Codex artifact.

`skill-field-dropped` entries follow the same lossiness metadata model as agent compilation. They are returned by lowering functions for tooling such as `mars validate --verbose`, but are not guaranteed to surface as user-visible warnings. The variant projection caller currently silences `Dropped` entries; only `Approximate` entries produce warnings.

---

## Skill Variants

A skill can provide harness-specific or model-specific body overrides in a `variants/` subdirectory. Variants replace only the **instruction body** — the base `SKILL.md` frontmatter is always authoritative for metadata.

### Layout

```
skills/<name>/
  SKILL.md                          # base content (frontmatter + body)
  variants/
    claude/
      SKILL.md                      # body override for Claude harness
      opus/SKILL.md                 # body override for Claude + opus model
    codex/
      SKILL.md                      # body override for Codex harness
      gpt55/SKILL.md                # body override for Codex + gpt55 model
```

Recognized harness keys: `claude`, `codex`, `opencode`, `pi`, `cursor`.

Model keys are directory names — they are matched exactly against the resolved model alias or canonical model ID at runtime.

### Compile-time projection (Mars)

When mars projects a skill to a native harness directory, it:

1. Copies the full skill tree, **excluding** the `variants/` subtree.
2. If a harness-level variant exists (`variants/<harness>/SKILL.md`), replaces the projected `SKILL.md` body with the variant's body.
3. Compiles the **base** frontmatter to harness-native fields.

The compiled native `SKILL.md` has: base frontmatter (lowered for target) + variant body (or base body if no variant).

Variant frontmatter is **not** used for metadata — it is ignored. Only the body of a variant file matters.

### Runtime selection (Meridian)

At launch time, Meridian reads from `.mars/skills/` and selects a variant body using a 4-step specificity ladder:

1. `variants/<harness>/<selected-model-alias>/SKILL.md` — model alias + harness
2. `variants/<harness>/<canonical-model-id>/SKILL.md` — canonical model ID + harness
3. `variants/<harness>/SKILL.md` — harness only
4. Base `SKILL.md` — default

Matching is exact-only at each step. The base skill's frontmatter is always used for metadata regardless of which body wins.

### Example

Source tree:

```
skills/my-skill/
  SKILL.md          # base: model-invocable: false, tools: [bash(git *)]
  variants/
    claude/
      SKILL.md      # Claude-specific instructions
    codex/
      SKILL.md      # Codex-specific instructions
```

After `mars sync`:

- `.mars/skills/my-skill/` — full fidelity, including `variants/`
- `.claude/skills/my-skill/SKILL.md` — Claude lowering of base frontmatter + claude variant body
- `.codex/skills/my-skill/SKILL.md` — Codex lowering of base frontmatter + codex variant body

Meridian at runtime: if resolving `claude+opus`, checks for `variants/claude/opus/SKILL.md` (not present) → falls back to `variants/claude/SKILL.md` → uses Claude variant body with base frontmatter.

---

## Canonical Store

`.mars/skills/` retains the universal schema — no lowering is applied here. Only native harness surfaces (`.claude/`, `.codex/`, etc.) receive harness-compiled frontmatter.

Meridian always reads from `.mars/skills/`. Skill compilation is transparent to the runtime; Meridian handles variant selection itself without re-compiling. Skill presence at boot is determined by the agent profile `skills:` list; skills do not define a `presence` field.

---

## Diagnostics

Mars emits diagnostics during `mars sync` and `mars validate` for skill compilation issues.
Field-loss diagnostics also surface via `mars check` and `mars init`, which run a
lossiness preview (`lossiness_preview::collect_source_lossiness_diagnostics`)
against the project's configured managed targets.

| Code | Severity | Cause |
|---|---|---|
| `skill-schema-error` | error | Invalid or malformed frontmatter, including removed `invocation`, `disable-model-invocation`, or `allow_implicit_invocation` fields; the parser-level variant for removed fields is `RemovedField` |
| `skill-field-dropped` | metadata | Internal lossiness metadata for fields with no native equivalent; used by verbose tooling, not a guaranteed user-visible warning |
| `skill-schema-warning` | warning | Non-fatal parse issue |
| `skill-variant-unknown-harness` | warning | Unknown harness key under `variants/` |
| `skill-variant-missing-skill` | warning | Model variant directory has no `SKILL.md` |

`skill-field-dropped` entries with `Dropped` lossiness are currently suppressed in normal projection output; only `Approximate` entries produce user-visible warnings.

The `skill-schema-warning` for non-canonical `allowed-tools` is a **Validation**
category diagnostic and is **not** gated by `surface_lossiness_warnings`. It is
always emitted when the condition is detected at the staging boundary, regardless
of whether field-loss warnings are configured.

Errors in frontmatter parsing skip frontmatter compilation for that skill; the body is still projected.
