# Skill Compilation

Skills use a universal frontmatter schema that mars compiles to per-harness native field formats during `mars sync`. This separates skill authoring from harness-specific field names — a skill author writes `model-invocable: false` once, and mars emits the right field for Claude, Codex, or any other target.

## Universal Skill Frontmatter

All skill fields below are optional. A skill with no frontmatter is valid and is treated as body-only with both model and user invocability enabled.

```yaml
---
name: my-skill
description: What this skill does
model-invocable: false
tools:
  bash(git *): allow
  read: allow
  write: allow
  mcp(plugin:demo): allow
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

Controls whether the skill appears in the harness's skill list, where a model can find and load it by itself.

```yaml
model-invocable: false   # not in the harness skill list; no model self-loads it
```

It gates discovery only. A skill an agent profile lists under `skills` (or `skills.load`), or that a launch adds with `--skill`, is loaded into that agent's launch bundle in full whatever this field says, so `model-invocable: false` plus a profile `load` gives one agent a skill no model finds on its own. A skill under `skills.available` is also listed whatever this field says, but the harness won't let the model load a non-invocable one, so avoid that pairing.

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
| Type | deprecated string[] **or** map of `tool-pattern: allow\|deny\|ask` |
| Default | empty |

Tools this skill pre-approves while it runs, and inline denials — the same rule grammar as agent profiles (see [agent-profiles.md](agent-profiles.md#tools)): Mars capability names (`bash`, `ask_user`, `web_search`), scoped patterns, readable aliases and native spellings such as `Bash`, `shell`, and `askuser`.

A skill's grants never restrict anything. `ask` has no skill field, so it is reported as
dropped lossiness.

List form is deprecated. It still grants the listed tools, restricts nothing, and emits a
warning. Write grants in map form instead:

```yaml
tools:
  bash(git *): allow
  read: allow
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
tools:
  bash(git *): allow
  mcp(plugin:context7): allow
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
| `model-invocable: false` | preserved | `disable-model-invocation: true` | sibling `openai.yaml`: `policy.allow_implicit_invocation: false`¹ | dropped | `disable-model-invocation: true` | `alwaysApply: false`, `description` removed² |
| `model-invocable: true` | preserved | (omit) | (omit) | (omit) | (omit) | `alwaysApply: false`, `description` kept |
| `user-invocable: false` | preserved | `user-invocable: false` | dropped | dropped | dropped | dropped |
| `user-invocable: true` | preserved | (omit) | (omit) | (omit) | (omit) | (omit) |
| `tools` | preserved | `allowed-tools` (+ projected MCP grants) | dropped | dropped | `allowed-tools` | dropped |
| `disallowed-tools` | preserved | `disallowed-tools` (+ projected MCP denials) | dropped | dropped | `disallowed-tools` | dropped |
| `mcp(...)` in `tools` / `disallowed-tools` | preserved | projected into `allowed-tools` / `disallowed-tools` | approximate | approximate | dropped | approximate |
| `license` | preserved | `license` | `license` | `license` | `license` | `license` |
| `metadata` | preserved | `metadata` | `metadata` | `metadata` | `metadata` | `metadata` |

¹ Codex gets `openai.yaml` beside the skill's `SKILL.md` (`policy:` with `allow_implicit_invocation: false`) only when the source skill explicitly sets `model-invocable: false`. An absent or `true` value writes nothing, because Codex allows implicit invocation by default. The field never appears in the Codex `SKILL.md`.

² Cursor rules have no invocation flag. A rule without a description is a Manual rule that Cursor's agent won't select by itself; a rule with one is selected when the description fits.

`skill-field-dropped` entries follow the same lossiness metadata model as agent compilation. They are returned by lowering functions for tooling such as `mars validate --verbose`, but are not guaranteed to surface as user-visible warnings. The variant projection caller currently silences `Dropped` entries; only `Approximate` entries produce warnings.

---

## Skill Variants

A skill can provide harness-specific body overrides in a `variants/` subdirectory. Variants replace only the **instruction body** — the base `SKILL.md` frontmatter is always authoritative for metadata.

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

Model-level directories (`variants/<harness>/<model>/`) are indexed and validated but never selected: neither sync nor the launch bundle chooses a body by model.

### Compile-time projection (Mars)

When mars projects a skill to a native harness directory, it:

1. Copies the full skill tree, **excluding** the `variants/` subtree.
2. If a harness-level variant exists (`variants/<harness>/SKILL.md`), replaces the projected `SKILL.md` body with the variant's body.
3. Compiles the **base** frontmatter to harness-native fields.

The compiled native `SKILL.md` has: base frontmatter (lowered for target) + variant body (or base body if no variant).

Variant frontmatter is **not** used for metadata — it is ignored. Only the body of a variant file matters.

### Launch-time selection (launch bundle)

`mars build launch-bundle` makes the same choice for each loaded skill: the body of `variants/<harness>/SKILL.md` when it exists, otherwise the base body. The bundle's `skills.loaded[].body` carries the selected body; `source_path` points at the base `SKILL.md`. Meridian composes that body into the agent's system prompt and does no variant selection of its own.

### Example

Source tree:

```
skills/my-skill/
  SKILL.md          # base: model-invocable: false, tools: {bash(git *): allow}
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

A launch bundle for a Claude agent that loads `my-skill` carries the Claude variant body, with `type` and other metadata from the base frontmatter.

---

## Canonical Store

`.mars/skills/` retains the universal schema — no lowering is applied here. Only native harness surfaces (`.claude/`, `.codex/`, etc.) receive harness-compiled frontmatter.

The launch bundle reads skills from `.mars/skills/`; Meridian gets skill content only through the bundle and never reads `.mars/skills/` itself on the launch path. Which skills an agent loads is decided by the agent profile's `skills:` and `--skill`, never by skill frontmatter; skills do not define a `presence` field.

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
