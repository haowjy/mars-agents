# Skill Compilation

Skills use a universal frontmatter schema that mars compiles to per-harness native field formats during `mars sync`. This separates skill authoring from harness-specific field names — a skill author writes `invocation: explicit` once, and mars emits the right field for Claude, Codex, or any other target.

## Universal Skill Frontmatter

All skill fields below are optional. A skill with no frontmatter is valid and is treated as body-only with implicit invocation.

```yaml
---
name: my-skill
description: What this skill does
invocation: explicit
allowed-tools: [Bash(git *), Read, Write]
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

### `invocation`

| | |
|---|---|
| Type | string |
| Allowed values | `explicit`, `implicit` |
| Default | `implicit` |

Controls whether an agent can call this skill spontaneously or only when explicitly told to.

| Value | Behavior |
|---|---|
| `implicit` | Agent may invoke the skill freely. Default. |
| `explicit` | Agent invokes the skill only when the user or orchestrator explicitly asks for it. |

```yaml
invocation: explicit   # require explicit call
invocation: implicit   # allow spontaneous use (default)
```

**Legacy aliases (deprecated).** These fields are recognized and converted to `invocation` during compilation, but should not be used in new source packages:

| Legacy field | Equivalent canonical field |
|---|---|
| `disable-model-invocation: true` | `invocation: explicit` |
| `disable-model-invocation: false` | `invocation: implicit` |
| `allow_implicit_invocation: true` | `invocation: implicit` |
| `allow_implicit_invocation: false` | `invocation: explicit` |

If `invocation:` is present, legacy fields are ignored (warning emitted). If both legacy fields are present and conflict, an error is emitted and `implicit` is used as fallback.

---

### `allowed-tools`

| | |
|---|---|
| Type | string[] |
| Default | empty |

Tool allowlist for this skill. Supports scoped patterns. Dropped by some harnesses — see the lossiness table below.

```yaml
allowed-tools: [Bash(git *), Read]
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

## Per-Harness Lowering

Mars compiles universal frontmatter fields to each target's native field names during `mars sync`. The `invocation` field in particular maps to different native fields per harness.

### Field mapping table

| Field | `.mars/` | Claude | Codex | OpenCode | Pi | Cursor |
|---|---|---|---|---|---|---|
| `name` | preserved | `name` | `name` | `name` | `name` | `name` |
| `description` | preserved | `description` | `description` | `description` | `description` | `description` |
| `invocation: explicit` | preserved | `disable-model-invocation: true` | `allow_implicit_invocation: false` | dropped | `disable-model-invocation: true` | `disable-model-invocation: true` |
| `invocation: implicit` | preserved | (omit field) | `allow_implicit_invocation: true`¹ | dropped | (omit field) | (omit field) |
| `allowed-tools` | preserved | `allowed-tools` | dropped | dropped | `allowed-tools` | dropped |
| `license` | preserved | `license` | `license` | `license` | `license` | `license` |
| `metadata` | preserved | `metadata` | `metadata` | `metadata` | `metadata` | `metadata` |

¹ Codex only emits `allow_implicit_invocation` when the source skill had an explicit `invocation` field or legacy invocation fields. Skills with no invocation field at all do not gain an `allow_implicit_invocation` field in the Codex artifact.

Lossiness diagnostics follow the same model as agent compilation:

```
warning[skill-field-dropped]: skill `my-skill`: field `allowed-tools` dropped in Codex native artifact
```

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
  SKILL.md          # base: invocation: explicit, allowed-tools: [Bash(git *)]
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

Meridian always reads from `.mars/skills/`. Skill compilation is transparent to the runtime; Meridian handles variant selection itself without re-compiling.

---

## Diagnostics

Mars emits diagnostics during `mars sync` and `mars validate` for skill compilation issues:

| Code | Severity | Cause |
|---|---|---|
| `skill-field-dropped` | warning | A field has no native equivalent in the target harness |
| `skill-schema-error` | error | Invalid or malformed frontmatter |
| `skill-schema-warning` | warning | Deprecated field or non-fatal parse issue |
| `skill-variant-unknown-harness` | warning | Unknown harness key under `variants/` |
| `skill-variant-missing-skill` | warning | Model variant directory has no `SKILL.md` |

Errors in frontmatter parsing skip frontmatter compilation for that skill; the body is still projected.
