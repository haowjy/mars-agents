# Skill Compilation

Skills use a universal frontmatter schema that mars compiles to per-harness native field formats during `mars sync`. This separates skill authoring from harness-specific field names — a skill author writes `model-invocable: false` once, and mars emits the right field for Claude, Codex, or any other target. The schema is [schema v1](schema-v1.md#skill-keys); a package that predates it can be upgraded with [`mars migrate`](migrating-to-schema-v1.md).

## Universal Skill Frontmatter

`name` and `description` are required when a skill has frontmatter; every other field is optional. A skill with no frontmatter is a body-only skill: it reads with a `skill.no-frontmatter` warning and has both model and user invocability enabled.

```yaml
---
name: my-skill
description: What this skill does
model-invocable: false
license: MIT
metadata:
  type: reference
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

---

### Removed fields

A skill carries no tool policy, and schema 1 rejects these keys with an error that names the replacement:

| Removed | Replace with |
|---|---|
| `type` | `metadata.type` |
| `when_to_use` | fold it into `description` |
| `tools`, `disallowed-tools`, `allowed-tools` and the snake_case spellings | nothing |
| `model_invocable`, `user_invocable` | `model-invocable`, `user-invocable` |
| `invocation`, `disable-model-invocation`, `allow_implicit_invocation` | `model-invocable` and `user-invocable` |

`mars migrate` makes these changes. A pre-v1 dependency gets the same upgrade in memory, and dropping a skill's tool keys warns with the file's name. Foreign skills (a Claude plugin's `allowed-tools`, for example) are translated when Mars imports them: the tool keys are dropped with a warning, because the skill itself carries no tool policy. See [Dialect](mars-toml.md#dialect).

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

Key-value metadata, passed through to all native artifacts. Mars reads one key, `metadata.type`, and leaves the rest to downstream tooling.

```yaml
metadata:
  type: principle
  owner: platform-team
```

**`metadata.type`** is a free-form string that classifies the skill. A value that is not a string is an error. It is **not** a restricted enum — any string is accepted and preserved. Mars uses it for exactly two things:

1. **Sort priority during prompt assembly.** Loaded skills are ordered by tier so higher-priority guidance lands first:

   | `metadata.type` | Priority |
   |---|---|
   | `principle` | 0 (first) |
   | `guardrail` | 1 |
   | `reference`, any other value, or unset | 2 (last) |

2. **Filtering.** `mars skills list --type <value>` matches `metadata.type` by exact string.

In the launch bundle, a skill with no `metadata.type` has `skill_type` `reference`. Custom values (such as `workflow`) sort at priority 2 and stay filterable by their exact string, but gain no behavior beyond `reference`.

---

## `harness-overrides`

A skill takes the same `harness-overrides` table as an Agent: a map of harness name to the native keys that harness's `SKILL.md` should carry. Mars writes the block for each harness into that harness's skill file (and plugin tree) and into no other. A key Mars lowers itself (`name`, `description`, `license`, `metadata`, `disable-model-invocation`, `user-invocable`, and Cursor's `alwaysApply`) keeps Mars's value; the override is dropped with a warning.

```yaml
---
name: my-skill
description: Does a thing
harness-overrides:
  claude:
    argument-hint: "What should I review?"
---
```

Importing a Claude skill moves Claude's own keys (`argument-hint`, `arguments`, `model`, `effort`, `context`, `agent`, `hooks`, `paths`, `shell`) here, unchanged and in file order, with an info message (`skill.claude-keys-moved`). `.claude/` keeps them; no other harness sees them. `model` and `effort` have schema meaning on Agents only, so a skill's are Claude's.

## Unknown Fields

A skill frontmatter field not listed above is kept in the authored file but written to **no** native file, so a Claude-only key never leaks into a Codex or OpenCode skill. Each one produces a `field.unknown` warning that says where harness-specific keys belong: `harness-overrides.<harness>`.

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
| `license` | preserved | `license` | `license` | `license` | `license` | `license` |
| `metadata` | preserved | `metadata` | `metadata` | `metadata` | `metadata` | `metadata` |
| unknown fields | preserved | dropped | dropped | dropped | dropped | dropped |
| `harness-overrides.<target>` | preserved | extra keys | extra keys | extra keys | extra keys | extra keys |

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
  SKILL.md          # base: model-invocable: false
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

A launch bundle for a Claude agent that loads `my-skill` carries the Claude variant body, with `skill_type` (from `metadata.type`) and other metadata from the base frontmatter.

---

## Canonical Store

`.mars/skills/` retains the universal schema — no lowering is applied here. Only native harness surfaces (`.claude/`, `.codex/`, etc.) receive harness-compiled frontmatter.

The launch bundle reads skills from `.mars/skills/`; Meridian gets skill content only through the bundle and never reads `.mars/skills/` itself on the launch path. Which skills an agent loads is decided by the agent profile's `skills:` and `--skill`, never by skill frontmatter; skills do not define a `presence` field.

---

## Diagnostics

Mars reads every skill with the schema reader and emits diagnostics during `mars sync`, `mars check` and `mars validate`.
Field-loss diagnostics also surface via `mars check` and `mars init`, which run a
lossiness preview (`lossiness_preview::collect_source_lossiness_diagnostics`)
against the project's configured managed targets.

| Code | Severity | Cause |
|---|---|---|
| `field.removed` | error | A removed key (see [Removed fields](#removed-fields)); the message names the replacement |
| `field.required` | error | `name` or `description` is missing or empty in a skill with frontmatter |
| `field.type` | error | A field has the wrong type, such as a non-string `metadata.type` or `model-invocable: yes` |
| `field.unknown` | warning | An unknown field; it stays in the file and reaches no native file |
| `skill.claude-keys-moved` | info | Import moved Claude-only keys to `harness-overrides.claude` |
| `skill.no-frontmatter` | warning | A body-only skill |
| `skill-field-dropped` | metadata | Internal lossiness metadata for fields with no native equivalent; used by verbose tooling, not a guaranteed user-visible warning |
| `skill-variant-unknown-harness` | warning | Unknown harness key under `variants/` |
| `skill-variant-missing-skill` | warning | Model variant directory has no `SKILL.md` |
| `skill-schema-error` | error | A selected variant's frontmatter is malformed |

A skill with an error fails `mars sync`, `mars check` and `mars validate` for its package before anything is written. `skill-field-dropped` entries with `Dropped` lossiness are currently suppressed in normal projection output; only `Approximate` entries produce user-visible warnings.
