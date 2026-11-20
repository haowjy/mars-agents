# Retired keys

Mars 0.34.0 dropped support for the format packages used before [schema v1](schema-v1.md), and with it `mars migrate`. A key an old Mars read and this one does not, is reported once as a `field.retired` warning that links here, and it stays in the file. A target directory it no longer writes (`.cursor`) is a `retired-target` warning on each sync. The warning is a warning because what the key set is lost: a `disallowed-tools` entry no longer denies a tool, a `permission` no longer restricts the Agent. Fix it by hand, using the entry below.

The list lives in one file, `crates/mars-schema/src/retired.rs`. A key that is not in it, or that was never Mars's, is a `field.unknown` `Info` instead.

A package that was migrated by Mars 0.33 or earlier has none of these keys. To convert one that was not, run `mars migrate` from Mars 0.33.x in the package's repository, then upgrade.

## Package without schema

A `[package]` table with no `schema` is an error: `manifest.schema-missing`. Mars 0.34.0 and later do not read the format that came before schema 1, so nothing in the package is installed. Add `schema = 1` to `[package]`, and read the entries below for every definition that still carries a retired key.

```toml
[package]
name = "my-package"
version = "1.0.0"
schema = 1
requires-mars = ">=0.20.0"
```

`requires-mars = ">=0.20.0"` is the first Mars that reads schema 1, so a consumer on an older Mars keeps the last release it can read. See [`requires-mars`](mars-toml.md#package-optional).

A dependency with no `mars.toml` is not a package of this kind: it is read as the dialect it looks like ([Dialects](mars-toml.md#dialect)).

## Package visibility

`[package] visibility` was removed. Delete it. Which models a consumer sees is the consumer's own setting.

## Host namespaces

`harness-overrides.flow` and `harness-overrides.meridian` became `harness-overrides.meridian-flow` and `harness-overrides.meridian-cli`, the full host names. Rename the block. The old block is not read, so what it set is lost until you do.

## Agent deny lists

`disallowed-tools`, `disallowed_tools` and `disallowedTools` became `deny` entries in `tools`. A deny list that is not carried over leaves the tool allowed.

```yaml
# before
disallowed-tools: [Bash]
# after
tools:
  bash: deny
```

A `tools` list (`tools: [read, grep]`) is an error (`tools.list-form`): schema 1 `tools` is a map of capability to `allow`, `deny` or `ask`. To restrict an Agent to a list, write an allowlist: `"*": deny` plus an `allow` per capability.

## Agent MCP tools

`mcp-tools` became `mcp(server)` or `mcp(server/tool)` entries in `tools`:

```yaml
tools:
  mcp(github): allow
```

## Agent underscore spellings

`model_invocable`, `user_invocable`, `network_allow` and `network_deny` became the hyphenated `model-invocable`, `user-invocable`, `network-allow` and `network-deny`. A `network_deny` that is not renamed leaves the hosts it listed reachable.

## Agent host keys

Keys that belonged to one host moved under that host's block in `harness-overrides`:

| Retired | Now |
|---|---|
| `permission: read` or `edit` | `harness-overrides.meridian-flow.permission` |
| `permission: {...}` (a map) | `harness-overrides.opencode.permission` |
| `resident-rearm-budget` | `harness-overrides.meridian-cli.resident-rearm-budget` |
| `meridian-capabilities` | `harness-overrides.meridian-cli.capabilities` |

## Agent fanout

`fanout` is the consumer's. Put the candidates in `[settings.meridian.fanout]` of the project's `mars.toml`.

## Agent models

`models` became `model-policies`; see [Model policies](schema-v1.md#model-policies).

## Agent autocompact

`autocompact` and `autocompact_pct` moved onto the model alias, in the consumer's `[models.<alias>]`.

## Skill type and when_to_use

`type: principle` became `metadata.type: principle`. `when_to_use` is folded into `description`, after a blank line.

## Skill tool policy

A skill carries no tool policy of Mars's own, so `tools` and `allowed_tools` have no schema 1 meaning: delete them. `allowed-tools`, `disallowed-tools` and `disallowedTools` are Claude's keys: move them under `harness-overrides.claude`. A tool restriction for every harness belongs on the Agent that uses the skill.

## Skill invocation

`disable-model-invocation: true` became `model-invocable: false`. `invocation` and `allow_implicit_invocation` have no replacement; delete them.

## managed_root

`mars init .claude` wrote `[settings] managed_root = ".claude"` before 0.37.0, and Mars 0.37.0 read it as `targets` while no file set `targets`. Mars no longer reads it, so a project that relied on it has no targets, and the project's routing is no longer limited to the harnesses it named. The first sync removes what earlier syncs installed in `.claude/` (copied skills, MCP servers, hook entries), as `mars unlink .claude` would; your own entries stay. `mars link .claude` installs it again. Each file that still has the key warns (`field.retired`) with the `targets` to write if you still want its target. `mars link` and `mars init <target>` remove the key from `mars.toml` when they write `targets`; `mars unlink` leaves it. Replace it with the same directory in `targets`:

```toml
# before
[settings]
managed_root = ".claude"
# after
[settings]
targets = [".claude"]
```

## Cursor

Mars no longer supports Cursor: `cursor` is not a harness and `.cursor` is not a target. What a project that names it gets:

- `cursor` or `.cursor` in `[settings] targets` is warned about on every sync (`retired-target`) and not synced to. Neither copied skills nor generated files are written there, and the files earlier syncs wrote stay where they are.
- `mars link cursor` fails. `mars unlink .cursor` (or taking `.cursor` out of `targets` and syncing) removes exactly what `mars.lock` records for it: the skills, agents and hook directories Mars copied, the MCP servers it added to `.cursor/mcp.json` and the hook entries it added to `.cursor/hooks.json`. A server or hook entry you wrote, and any other file in `.cursor/`, stays. An entry you edited after Mars wrote it also stays. If `.cursor/hooks.json` held only Mars's entries, it is left holding `{"version": 1}`: delete it by hand.
- `cursor` as `[models.<alias>] harness`, `default_harness` or an entry of `harness_order` is a `field.retired` warning on every sync: naming it has no effect, so delete it. A sync removes what it installed in a `.cursor` that `targets` no longer names, as for any target the config does not name.
- `harness-overrides.cursor` is a retired key (`field.retired`): the block has no effect.
- `variants/cursor/` in a skill is an unknown harness variant, which Mars reports and does not read.
- A hook's `[targets.".cursor"]` table and a `cursor.json` native hook name a target that is not managed, so they are ignored like any target the project does not link.
- A `cursor` entry in `harness_order` or `provider_order` is reported and skipped.
- `harness: cursor` in an Agent profile is a retired value (`field.retired`): the Agent pins no harness. `harness = "cursor"` in a `[models.<alias>]` table is read as no harness, so the alias routes as if it named none, and a `[[show]]` or `[[hide]]` rule in `mars.curated.toml` for harness `cursor` is dropped, because it can match no model.
- `mars models --harness cursor`, `mars build launch-bundle --harness cursor` and `harnesses = ["cursor"]` in `[plugin]` are errors: `cursor` is an unknown harness name.
- `dialect = "cursor"` still reads a package laid out as a Cursor repository (`.cursor/rules/*.mdc`, `.cursor-plugin/`); the keys it rejects are dropped, since no harness reads them.

## Values that changed

A value of a key Mars reads is checked against the keys it allows, so these are ordinary errors that list the allowed values: `approval: yolo` is now `approval: never`, and `effort: adaptive` and `effort: disabled` are now `effort: auto` and `effort: none`, in a profile and in a model policy's `override`.

## An Agent with no frontmatter

An Agent whose first line is not exactly `---` has no frontmatter, so every field takes its default and it reads as the widest Agent. Schema 1 refuses it (`frontmatter.missing`), in a package and in a project's own `.mars-src/agents`. Put two `---` lines above a body-only Agent's text, or delete the stray line above the fence.
