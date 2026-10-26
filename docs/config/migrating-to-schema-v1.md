# Migrating a package to schema v1

A package written before [schema v1](schema-v1.md) has no `schema` key in its `[package]` table and may use keys schema 1 removed. `mars migrate` rewrites its definitions in place.

```bash
cd my-package
mars migrate --check    # list the files that would change; exit 1 if there are any
mars migrate            # rewrite them
mars check              # confirm the package is clean
```

`mars migrate` deletes some keys without a replacement and prints each one after the files it edited, grouped by code (`field.removed`, `skill.tools-dropped`), one line per file and key, none left out. `--json` carries the same list as `dropped`. Review the result with `git diff` before you commit.

A value with no schema 1 meaning stops the run and names the file and key (``agents/b.md: agent field `model-policies[0].override.effort` has invalid value `bogus` ``). The legacy `effort` values `adaptive` and `disabled` are mapped to `medium` and `none`, in `effort:` and in a model policy's `override.effort`.

## What it changes

| File | Before | After |
|---|---|---|
| `mars.toml` | no `schema` | `schema = 1` in `[package]` (and `package.visibility`, if present, removed) |
| Agent | `tools: [read, grep]` | `tools: {read: allow, grep: allow}` |
| Agent | `disallowed-tools: [agent]` | `agent: deny` merged into `tools` |
| Agent | `approval: yolo` | `approval: never` |
| Agent | `effort: adaptive`, `effort: disabled` | `effort: medium`, `effort: none` |
| Agent | `permission: read` (or `edit`) | `harness-overrides.flow.permission: read` |
| Agent | `model_invocable`, `user_invocable`, `disallowed_tools` | the hyphenated spelling |
| Agent | `network`, `network-allow`, `network-deny`, `autocompact`, `autocompact_pct`, `fanout`, `models`, `mcp-tools` | **deleted**, with no replacement written |
| Agent | `network`, `autocompact`, `autocompact_pct` or `harness` inside a `model-policies` `override` | **deleted** |
| Skill | `type: principle` | `metadata.type: principle` |
| Skill | `when_to_use: ...` | appended to `description` after a blank line |
| Skill | `tools`, `disallowed-tools`, `allowed-tools` (and snake_case spellings) | **deleted** |
| Skill | `model_invocable`, `user_invocable` | the hyphenated spelling |
| Agent, skill | a key only Claude reads (Agent: `color`, `maxTurns`, `permissionMode`, `memory`, `background`, `isolation`, `initialPrompt`, `hooks`; skill: `argument-hint`, `arguments`, `model`, `effort`, `context`, `agent`, `hooks`, `paths`, `shell`) | moved to `harness-overrides.claude`, so `.claude/` keeps it and the other harnesses stop receiving it |

Where a deleted key held something you still want, put it where schema 1 keeps it: compaction on the model alias (`[models.<alias>] autocompact = ...`), fan-out candidates in the consumer's `[settings.meridian.fanout]`, MCP grants as `mcp(server): allow` entries in `tools`, `models` as `model-policies`. `network` has no replacement because no consumer acted on it.

An Agent that sets `tools` as a list with only some names used to restrict nothing; the map form `{name: allow}` states the same grants. It still restricts nothing. To restrict an Agent to a list, write the allowlist form: `"*": deny` plus an `allow` per capability.

## An example

`mars migrate` on `meridian-base` 0.11.18 changed 25 files (+47, −24): its manifest, one Agent and 23 skills. The three kinds of change:

```diff
--- a/mars.toml
+++ b/mars.toml
@@ -4,6 +4,7 @@ version = "0.11.18"
 description = "Core coordination primitives for Meridian — agent profiles and skills for spawning, state tracking, and session context."
 requires-mars = ">=0.18.0"
 requires-meridian = ">=0.14.0"
+schema = 1
 
 [dependencies]
 
--- a/agents/kb-maintainer.md
+++ b/agents/kb-maintainer.md
@@ -40,7 +40,6 @@ tools:
   'bash(git clean:*)': deny
   ask_user: deny
 sandbox: workspace-write
-network: allow
 ---
--- a/skills/grill/SKILL.md
+++ b/skills/grill/SKILL.md
@@ -1,11 +1,12 @@
 ---
 name: grill
-type: mode-shift
 description: >
   Use in conversation to stress-test a plan or direction. Interviews the
   human in rounds, asking every question that is ready with a recommended
   answer, until no decision is left open.
 model-invocable: true
+metadata:
+  type: mode-shift
 ---
```

## It edits minimally

The upgrade changes the top-level keys that need it and leaves every other byte alone: comments, quoting, key order, blank lines and the body. A definition with nothing to change is not written. In the example above the 23 skills changed by the lines shown and no other line moved.

A changed key's block is written again in YAML's standard style, so a key that changed can lose its original layout (a flow map such as `{alias: opus}` becomes a block mapping). When a definition's top level is not a block mapping, or the edited text fails Mars's check that it reads the same as the upgraded value, Mars rewrites the whole frontmatter instead and reports the file as `fully rewrote`; `--json` lists those files under `fully_rewritten_files`.

Running it again is a no-op: `definition format is already schema v1`.

`mars migrate` never touches `.mars/`, `mars.lock` or dependencies. Installed content is already schema 1.

## Consumer projects

A project with no `[package]` table installs packages and publishes none. `mars migrate` there upgrades only the project's own `.mars-src/` definitions (until then `mars sync` upgrades them in memory and warns: ``authored `.mars-src` content uses the pre-v1 format (2 files to upgrade); run `mars migrate` ``); with none, it exits 0 and says `nothing to migrate: ... is consumer configuration, not a package; dependencies migrate in their own repositories`.

The project's own consumer tables (`[settings]`, `[models]`, `[agents.<name>]`, `[skills.<name>]`) are not part of the schema and are not rewritten. Keys in them that Mars no longer reads, such as `default_network` or `network` in an overlay, are ignored and warned about by name; delete them. An invalid overlay `effort`, `approval` or `sandbox` is an error. A `[skills.<name>.tools]` table is ignored too. See [Overlays](mars-toml.md#agentsname-overlay).

## A dependency that has not migrated

A dependency without `schema = 1` still installs. Mars upgrades its definitions in memory with the same rules as `mars migrate`, installs the result into `.mars/`, and warns once per package:

```text
warning: package `legacy` uses the pre-v1 format (3 files to upgrade); run `mars migrate` in its repository
```

The warning names the package and how many of its files the upgrade changes, so you can ask its maintainer to migrate; nothing in your project needs to change, and the dependency's repository is not modified. The keys the upgrade dropped or moved, file by file, are listed only by `mars migrate` in the package's own repository.

A dependency that declares `schema = 1` and breaks its rules is different: Mars reports each error with the file and the replacement and fails `mars sync` before it writes anything. A tree with no `mars.toml` is upgraded in memory like a pre-v1 package when it is declared `dialect = "mars-native"` or carries a key only pre-v1 Mars defined (`network`, `network-allow`, `network-deny`, `fanout`, `models`, `mcp-tools`, `autocompact`, `autocompact_pct`, `permission`, `model_invocable`, `user_invocable`) or `approval: yolo`. Any other tree with no `mars.toml` is read under [Claude's rules](mars-toml.md#dialect), or the dialect it is declared as, which translate its definitions. What the translation cannot make schema v1 is moved under `harness-overrides.<dialect>` or dropped with a warning that names it, so an OpenCode agent with `tools: {write: false}` or a `permission` map installs. The same holds for a pre-v1 file: a value no schema v1 field accepts (`effort: ultra`) moves to `harness-overrides.flow`, beside the `permission` the upgrade already moves there.

## `mars migrate` and `mars upgrade`

They share nothing. `mars upgrade` moves your dependencies to newer versions within their constraints. `mars migrate` rewrites the files of the package in front of you to the current definition format.

## After migrating

Commit the result and release it. A package that now declares `schema = 1` needs a Mars that reads schema 1, so raise its `requires-mars` to the first Mars release that does: consumers on older Mars versions then keep resolving the last release that they can read ([`requires-mars`](mars-toml.md#package-optional)).
