# Retired keys

Mars 0.34.0 dropped support for the format packages used before [schema v1](schema-v1.md), and with it `mars migrate`. A key an old Mars read and this one does not is reported once as a `field.retired` warning that links here, and it stays in the file. The warning is a warning because what the key set is lost: a `disallowed-tools` entry no longer denies a tool, a `permission` no longer restricts the Agent. Fix it by hand, using the entry below.

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

## Values that changed

A value of a key Mars reads is checked against the keys it allows, so these are ordinary errors that list the allowed values: `approval: yolo` is now `approval: never`, and `effort: adaptive` and `effort: disabled` are now `effort: auto` and `effort: none`, in a profile and in a model policy's `override`.

## An Agent with no frontmatter

An Agent whose first line is not exactly `---` has no frontmatter, so every field takes its default and it reads as the widest Agent. Schema 1 refuses it (`frontmatter.missing`), in a package and in a project's own `.mars-src/agents`. Put two `---` lines above a body-only Agent's text, or delete the stray line above the fence.
