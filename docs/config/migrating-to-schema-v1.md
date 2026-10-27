# Migrating a package to schema v1

A package written before [schema v1](schema-v1.md) has no `schema` key in its `[package]` table and may use keys schema 1 removed. `mars migrate` rewrites its definitions in place.

```bash
cd my-package
mars migrate --check    # list the files that would change; exit 1 if there are any
mars migrate            # rewrite them
mars check              # confirm the package is clean
```

`mars migrate` deletes some keys without a replacement and prints each one after the files it edited, grouped by code (`field.removed`, `skill.tools-dropped`, `field.set-aside`, `field.restriction-lost`), one line per file and key, none left out. A key that restricted an agent (a `deny` or `ask` tool entry, `permission`, `sandbox`, `approval`) and has no schema 1 meaning is listed under `field.restriction-lost`, in a package and in a project's `.mars-src` alike; `mars sync` prints the same line for a pre-v1 dependency on every run. `--json` carries the same list as `dropped`. Review the result with `git diff` before you commit.

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
| Agent | `resident-rearm-budget: 3` | `harness-overrides.meridian.resident-rearm-budget: 3` |
| Agent | `meridian-capabilities: {...}` | `harness-overrides.meridian.capabilities: {...}` |
| Agent | `model_invocable`, `user_invocable`, `disallowed_tools`, `network_allow`, `network_deny` | the hyphenated spelling |
| Agent | `autocompact`, `autocompact_pct`, `fanout`, `models`, `mcp-tools` | **deleted**, with no replacement written |
| Agent | `autocompact`, `autocompact_pct` or `harness` inside a `model-policies` `override` | **deleted** |
| Skill | `type: principle` | `metadata.type: principle` |
| Skill | `when_to_use: ...` | appended to `description` after a blank line |
| Skill | `tools`, `disallowed-tools`, `allowed-tools` (and snake_case spellings) | **deleted** |
| Skill | `model_invocable`, `user_invocable` | the hyphenated spelling |
| Agent, skill | a key only Claude reads (Agent: `color`, `maxTurns`, `permissionMode`, `memory`, `background`, `isolation`, `initialPrompt`, `hooks`; skill: `argument-hint`, `arguments`, `model`, `effort`, `context`, `agent`, `hooks`, `paths`, `shell`) | moved to `harness-overrides.claude`, so `.claude/` keeps it and the other harnesses stop receiving it |

Where a deleted key held something you still want, put it where schema 1 keeps it: compaction on the model alias (`[models.<alias>] autocompact = ...`), fan-out candidates in the consumer's `[settings.meridian.fanout]`, MCP grants as `mcp(server): allow` entries in `tools`, `models` as `model-policies`. `network`, `network-allow` and `network-deny` are schema 1 keys, so a profile that sets them keeps them.

The three moves to a `harness-overrides` namespace are the only repairs `mars migrate` makes to a package that already says `schema = 1`: a `permission` of `read` or `edit`, `resident-rearm-budget` or `meridian-capabilities` left at the top level of a migrated file is moved with its value, the rest of the file keeps its key order, and `harness-overrides` keeps the place the author gave it. A block that is already set there wins; the top-level twin is dropped with a line naming it. Every other key schema 1 removed still fails the run and names the file and the key.

An Agent that sets `tools` as a list with only some names used to restrict nothing; the map form `{name: allow}` states the same grants. It still restricts nothing. To restrict an Agent to a list, write the allowlist form: `"*": deny` plus an `allow` per capability.

## An example

`mars migrate` on `meridian-base` 0.11.18 changed 24 files (+56, −26): its manifest and 23 skills. Two of those changes:

```diff
--- a/mars.toml
+++ b/mars.toml
@@ -4,6 +4,7 @@ version = "0.11.18"
 description = "Core coordination primitives for Meridian — agent profiles and skills for spawning, state tracking, and session context."
 requires-mars = ">=0.18.0"
 requires-meridian = ">=0.14.0"
+schema = 1
 
 [dependencies]
 
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

The upgrade changes only the top-level keys that need it; everything else stays as authored: quoting, key order, blank lines and the body. A definition with nothing to change is not written. In the example above `requires-mars` keeps its authored value and each file changed on the lines the table above names and no other line moved. In `mars.toml`, migrate adds `schema = 1` and a missing `requires-mars = ">=0.20.0"`; it drops the removed `visibility`. It never rewrites a requirement that is already there: one that allows a Mars older than 0.20.0 gets the same warning `mars check` prints, naming the file, and keeps its value and comment for the author to decide. A manifest migrate cannot edit minimally, such as an inline `package = { ... }` table, is left as authored with that warning instead of failing, so a package already at `schema = 1` migrates exactly as before. `mars migrate` adds the floor to a package an earlier v1 Mars migrated, which needs no hand edit.

A top-level key's block runs from its own line to the line before the next key that starts a line. Indented lines, blank lines and comment lines in between belong to the block above, so a comment you meant for the next key and wrote directly after a changed key's value is rewritten with that block and lost. A comment after a key that does not change survives, and so does anything above the first key.

A changed key's block is written again in YAML's standard style, so a key that changed can lose its original layout (a flow map such as `{alias: opus}` becomes a block mapping). The one exception is a `harness-overrides` block that gains an entry: migrate appends the entry as text, so the comments, flow style and indentation you wrote in the entries already there survive. When it cannot append — the block is written in flow style, or an entry it already has is the one that changes — it re-renders the block and reports the file as `fully rewrote`, because what you wrote inside it is lost. When a definition's top level is not a block mapping, or the edited text fails Mars's check that it reads the same as the upgraded value, Mars re-renders the whole frontmatter instead and reports the file as `fully rewrote`; `--json` lists those files under `fully_rewritten_files`. That rewrite loses the comments inside the frontmatter, but it copies the body from the input byte for byte, including a BOM and any mixed line endings.

Running it again is a no-op: `definition format is already schema v1`.

`mars migrate` never touches `.mars/`, `mars.lock` or dependencies. Installed content is already schema 1.

## Consumer projects

A project with no `[package]` table installs packages and publishes none. `mars migrate` there upgrades only the project's own `.mars-src/` definitions (until then `mars sync` upgrades them in memory and warns: ``authored `.mars-src` content uses the pre-v1 format (2 files to upgrade); run `mars migrate` ``); with none, it exits 0 and says `nothing to migrate: ... is consumer configuration, not a package; dependencies migrate in their own repositories`.

The project's own consumer tables (`[settings]`, `[models]`, `[agents.<name>]`, `[skills.<name>]`) are not part of the schema and are not rewritten. Keys in them that Mars no longer reads, such as `autocompact` in an overlay, are ignored and warned about by name; delete them. An invalid overlay `effort`, `approval` or `sandbox` is an error. A `[skills.<name>.tools]` table is ignored too. See [Overlays](mars-toml.md#agentsname-overlay).

## A dependency that has not migrated

A dependency without `schema = 1` still installs. Mars upgrades its definitions in memory with the same rules as `mars migrate`, installs the result into `.mars/`, and warns once per package:

```text
warning: package `legacy` uses the pre-v1 format (3 files to upgrade); run `mars migrate` in its repository
```

The warning names the package and how many of its files the upgrade changes, so you can ask its maintainer to migrate; nothing in your project needs to change, and the dependency's repository is not modified. The keys the upgrade dropped or moved, file by file, are listed only by `mars migrate` in the package's own repository.

An installed `.mars/` tree written by an older Mars still reads after a Mars upgrade, before the next `mars sync`. A file whose strict reading reports a pre-v1 spelling (a removed key or value, or a `tools` list) is upgraded in memory by the same rules, so a skill's `type` or an Agent's `model_invocable` does not drop the item from a launch bundle; `mars sync` then installs the schema v1 text. A package that declares `schema = 1` is still read strictly, because that rule is about the authored package, not about what Mars wrote into `.mars/`.

A tree with no `mars.toml` is read as the dialect it looks like, or the one the dependency declares ([Claude's rules](mars-toml.md#dialect) unless an Agent is written the way only OpenCode writes one), and what only pre-v1 Mars wrote is rewritten key by key in whichever dialect that is. `model_invocable`, `user_invocable`, `network_allow` and `network_deny` are renamed (a host set written under both spellings keeps the hosts of both), and `network`, `network-allow` and `network-deny` stay; the fields Mars removed without a replacement (`fanout`, `models`, `mcp-tools`, `autocompact`, `autocompact_pct`, and the removed model-policy override keys) are dropped; `approval: yolo`, `effort: adaptive` and `effort: disabled` are rewritten; and a `permission` of `read` or `edit` moves to `harness-overrides.flow.permission`, as do `resident-rearm-budget` and `meridian-capabilities` to `harness-overrides.meridian`. One such key never changes how its neighbours are read: a pre-v1 Mars agent with `tools: [Read, Grep]` becomes `read: allow` and `grep: allow`, plain grants that restrict nothing, as the list form never restricted anything either, and a Claude agent beside a pre-v1 Mars skill keeps its `tools` string. `mars sync` shows the count of files translated; to migrate the dependency for good, its maintainer adds a `mars.toml` with a `[package]` table (name and version, no `schema`) and runs `mars migrate`. `[package] schema = 1` declares the package already migrated, so `mars migrate` changes only the keys it can move to a `harness-overrides` namespace, and every other old key is an error.

A tree with no `mars.toml` that is declared `dialect = "mars-native"` is a pre-v1 package, upgraded in memory with its own wording, because `mars migrate` needs a manifest to find the package:

```text
warning: package `legacy` has no `mars.toml` and is declared `mars-native` (1 file to upgrade); add a `mars.toml` with a `[package]` table (name and version, no `schema`) and run `mars migrate` in its repository
```

A tree that is already schema v1 (a `mars-native` declaration over current definitions) installs with no warning.

A dependency that declares `schema = 1` and breaks its rules is different: Mars reports each error with the file and the replacement and fails `mars sync` before it writes any installed content. A translated or upgraded definition is never refused: what it cannot make schema v1 is moved under `harness-overrides.<dialect>` or dropped with a warning that names it, so an OpenCode agent with `tools: {write: false}` or a `permission` map installs. A key Mars lowers itself (an Agent's `model`, `effort`, `tools`, `disallowed-tools`, `skills` and `mode`; a skill's `license`, `metadata` and `user-invocable`) is never moved there, because a native file would keep Mars's value and drop the override: an invalid entry of `tools` is dropped and the rest kept, and an invalid scalar such as `effort: ultra` is dropped. A pre-v1 file's leftovers are dropped the same way, not written to `harness-overrides.flow` or `harness-overrides.meridian`, which hold only their consumer's own keys. Dropping or moving a restriction (a `deny` or `ask` tool entry, `permission`, `sandbox`, `approval`) is a warning on its own file that names the agent, and is not folded into the count.

## `mars migrate` and `mars upgrade`

They share nothing. `mars upgrade` moves your dependencies to newer versions within their constraints. `mars migrate` rewrites the files of the package in front of you to the current definition format.

## After migrating

Commit the result and release it. `mars migrate` also adds `[package] requires-mars = ">=0.20.0"`, the first Mars release that reads schema 1, when the package has no requirement. That keeps consumers on older Mars versions resolving the last release they can read ([`requires-mars`](mars-toml.md#package-optional)).

An existing `requires-mars` is never rewritten, because it is the author's decision which Mars versions may read the package. If it allows one older than 0.20.0, `mars migrate` prints the warning `mars check` prints and leaves the value, its comments and any upper bounds alone. Raise it by hand when the package can drop older consumers; `mars check` keeps warning until you do.
