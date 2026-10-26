# CLI Reference

All commands support two global flags:

| Flag | Description |
|---|---|
| `--root <PATH>` | Explicit managed root (default: auto-detect from cwd) |
| `--json` | Machine-readable JSON output |

The managed root's parent directory is the project root (where `mars.toml` and `mars.lock` live).

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | `check`: validation errors found; `migrate --check`: files need migration; `resolve`: unresolved conflict markers remain |
| `2` | Config, resolution, validation, lock, request, collision, or frozen-violation errors, including a definition that breaks [schema v1](../config/schema-v1.md) |
| `3` | I/O, source fetch, HTTP, or git CLI errors |

---

## `mars init`

Initialize a mars project. Creates `mars.toml` and the managed output directory.

```bash
mars init [TARGET] [--link DIR...]
```

| Argument/Flag | Description |
|---|---|
| `TARGET` | Directory name for managed output (default: `.agents`) |
| `--link DIR` | Directories to link after init (repeatable) |

**Behavior:**
- Creates `<project-root>/mars.toml` with `[dependencies]` section
- Creates the managed directory (default `.agents/`)
- Creates `.mars/` internal state directory
- Adds `mars.local.toml` and `.mars/` to `.gitignore`
- Persists non-default `managed_root` in `[settings]`
- Idempotent: re-running is a no-op for init, but still processes `--link`
- When the project's `mars.toml` declares `[package]`, runs a **field-loss preview**
  against configured managed targets, reporting what each target cannot express
  (lossiness diagnostics via `collect_prepared_source_lossiness_diagnostics`). A consumer
  project has no package to publish, so it gets no preview

```bash
mars init                    # Default: .agents/
mars init .claude            # Custom managed root
mars init --link .claude     # Init + link in one step
```

---

## `mars add`

Add one or more dependencies, then sync.

```bash
mars add <source...> [filter flags]
```

| Flag | Description |
|---|---|
| `--subpath PATH` | Install from a package rooted under a subdirectory (requires exactly one source) |
| `--plugin NAME` | Install one plugin from the Claude marketplace at the source (requires exactly one source; cannot be combined with `--subpath`) |
| `--agents NAME,...` | Install only named agents (include mode) |
| `--skills NAME,...` | Install only named skills (include mode) |
| `--exclude NAME,...` | Exclude named items |
| `--only-skills` | Install only skills from this source |
| `--only-agents` | Install only agents + transitive skill dependencies (skills those agents declare in frontmatter) |

**Rules:**
- `--subpath` and `--plugin` require exactly one source.
- A marketplace source with exactly one plugin installs it without `--plugin`. With several, `--plugin` is required: the error lists them and the dependency is not written.
- Filter flags require exactly one source. Multi-source add is only for whole-source installs.
- Re-adding an existing dependency updates it. Filter fields are replaced atomically when any filter flag is present; version-only changes preserve existing filters.
- Source specifiers:
  - GitHub: `owner/repo`, `owner/repo/plugins/foo`, `github:owner/repo`, `https://github.com/owner/repo/tree/main/plugins/foo`
  - GitLab: `gitlab:group/subgroup/repo`, `https://gitlab.example.com/group/subgroup/repo`, `https://gitlab.example.com/group/subgroup/repo/-/tree/main/plugins/foo`
  - Generic git: `git@example.com:org/repo.git`, `git://host/org/repo.git`
  - Local paths
- A dependency whose definitions break schema v1 is rejected before anything is written: the command exits 2 and `mars.toml` does not gain the dependency (see [sync](#mars-sync)).
- Archive-download and direct file-download URLs are rejected in v1.

Dependency naming model:
- `mars add` uses the source specifier to derive a dependency name (for example, `meridian-flow/meridian-base` -> `meridian-base`).
- That dependency name is the key stored in `mars.toml` under `[dependencies.<NAME>]`.
- A marketplace plugin is named `<repo>/<plugin>` (`anthropics/skills` with `--plugin document-skills` -> `skills/document-skills`), as a subpath package is named `<repo>/<subpath>`.
- Subsequent commands use that dependency name, not the source URL.

```bash
mars add meridian-flow/meridian-base
mars add meridian-flow/meridian-base@^1.0
mars add meridian-flow/meridian-base/plugins/foo
mars add gitlab:group/subgroup/repo --subpath plugins/foo
mars add ../my-local-agents
mars add ../monorepo --subpath packages/agents
mars add acme/ops --agents deployer,monitor
mars add acme/toolkit --only-skills
mars add anthropics/skills --plugin document-skills
mars add source1 source2          # Multi-source (no filters)
```

---

## `mars remove`

Remove a dependency and prune its installed items.

```bash
mars remove <name>
```

Removes the named dependency from `mars.toml` and runs sync to clean up installed items. Whole-source removal only; to narrow a source's installed items, use `mars add` with updated filters.

A dependency added with `--subpath`, `--plugin` or a path in the specifier is named `<repository>/<path or plugin>` (`mars add anthropics/skills --plugin claude-api` is `skills/claude-api`). `mars remove skills` finds nothing and names the dependencies that start with `skills/`; remove one by its full name.

---

## `mars sync`

Resolve dependencies and make the managed root match config.

```bash
mars sync [--force] [--diff] [--frozen]
```

| Flag | Description |
|---|---|
| `--force` | Overwrite local modifications for managed files |
| `--diff` | Dry run: show what would change without applying |
| `--frozen` | Error if lock file would change (CI mode) |

This is the core operation. It runs the full [sync pipeline](../internals/sync-pipeline.md): resolve → prepare → target → diff → apply. Mars reads every dependency's and the project's own definitions as [schema v1](../config/schema-v1.md) before it writes anything. An error (`field.removed`, `field.value`, `field.type`, `field.required`, `tools.list-form`, `yaml.*` and `frontmatter.*`) fails the sync with exit 2 and leaves `.mars/`, the targets and `mars.lock` as they were. Warnings (`field.unknown`, the lossiness a target cannot express) are printed and the sync goes on. A dependency whose `[package]` has no `schema` is upgraded in memory and installed, with one warning per package that names it; ask its maintainer to run [`mars migrate`](#mars-migrate). Native agent materialization follows `settings.agent_emission`; `[settings.meridian.agent_copy]` can selectively emit qualifying harness-native agent copies even when emission is otherwise suppressed.

---

## `mars upgrade`

Upgrade dependencies to newest versions within their constraints.

```bash
mars upgrade [names...]
```

| Argument | Description |
|---|---|
| `names` | Specific dependency names to upgrade (default: all) |

Uses maximize strategy instead of minimum version selection. Only git sources with semver constraints are upgradeable; path sources have no version to maximize. It moves dependency versions and never edits definitions: for that, see [`mars migrate`](#mars-migrate).

A marketplace plugin is refreshed as a whole: Mars re-reads the marketplace at its ref, takes the plugin's current source from it and resolves the plugin's content commit anew, even when the marketplace itself did not change. A plugin whose source moved to another repository or path is reported (`plugin <name> of <dependency> moved: <old> -> <new>`).

```bash
mars upgrade                 # Upgrade all
mars upgrade base dev        # Upgrade specific deps
```

---

## `mars migrate`

Rewrite the package's own definitions to [schema v1](../config/schema-v1.md), in place.

```bash
mars migrate [--check] [--root PATH] [--json]
```

| Flag | Description |
|---|---|
| `--check` | List the files that would change and write nothing. Exits 1 if any file needs migration |
| `--json` | `{check, changed, files, fully_rewritten_files, dropped}`; `dropped` lists `{file, code, message}` for each key the migration removed |

It reads `mars.toml` for the package it is in, upgrades `[package]` (adds `schema = 1`, drops `visibility`) and every agent and skill definition, and writes only the files that change. If a definition still has an error after the upgrade (a value it cannot map, malformed YAML), it exits 2 and writes nothing. Comments, quoting, key order and the body of a definition stay as they were; only the top-level keys that change are written again. A second run prints `definition format is already schema v1`.

In a consumer project (a `mars.toml` with no `[package]`), only `.mars-src/` is migrated; with none, the command says `nothing to migrate: ... is consumer configuration, not a package; dependencies migrate in their own repositories` and exits 0.

It never touches `.mars/`, `mars.lock` or dependencies, so it is a different job from [`mars upgrade`](#mars-upgrade), which moves dependency versions. It deletes some keys without a replacement and prints each one after the files it edited, grouped by code with one line per file and key; read `git diff` as well. The rewrite table, an example diff and the handling of unmigrated dependencies are in [Migrating a package to schema v1](../config/migrating-to-schema-v1.md).

---

## `mars outdated`

Show available updates without applying them.

```bash
mars outdated
```

Displays a table with columns: SOURCE, LOCKED, CONSTRAINT, UPDATEABLE, LATEST.

- For tagged repos: compares locked version against available tags
- For untagged repos: compares locked commit against current HEAD
- Path sources are skipped (no version to check)

---

## `mars list`

List installed agents and skills.

```bash
mars list [--source NAME] [--kind KIND] [--status]
```

| Flag | Description |
|---|---|
| `--source NAME` | Filter by dependency name |
| `--kind KIND` | Filter by item kind (`agents` or `skills`) |
| `--status` | Show detailed status (source, version, hash check) |

Default view shows name + description from frontmatter (the YAML metadata block at the top of each Markdown profile). With `--status`, output includes the dependency source name, version, and integrity status (`ok`, `modified`, `missing`, `conflicted`).

---

## `mars why`

Explain why an item is installed.

```bash
mars why <name>
```

Shows the item's source dependency name, version, install path, and which agents depend on it (for skills). Matches by name stem (agents) or directory name (skills).

```bash
mars why frontend-design
# frontend-design (skill)
#   provided by: base@v1.2.0
#   installed at: skills/frontend-design
#   required by:
#     agents/frontend-coder.md
```

---

## `mars rename`

Rename a managed item by adding a rename mapping to its source config.

```bash
mars rename <from> <to>
```

| Argument | Description |
|---|---|
| `from` | Current managed item path (e.g., `agents/old-coder.md`) |
| `to` | Desired item path (e.g., `agents/coder.md`) |

Adds a `rename` entry to the dependency's config in `mars.toml` and re-syncs.
Cross-source agent/skill collisions auto-rename both colliders with
`__{source_name}`; use `rename` when you want a custom installed name or want to
avoid the automatic suffix.

---

## `mars adopt`

Move an unmanaged item from a target directory into `.mars-src/`, then sync.

```bash
mars adopt <path> [--dry-run]
```

| Argument/Flag | Description |
|---|---|
| `path` | Path to an unmanaged agent file or skill directory inside a managed target |
| `--dry-run` | Show what would happen without moving anything or syncing |

**Behavior:**
- Validates that `path` is inside a managed target directory and is not already tracked by Mars
- Agent files (`.md` inside `agents/`) are moved to `.mars-src/agents/<name>.md`
- Skill directories (containing `SKILL.md`) are moved to `.mars-src/skills/<name>/`
- Runs `mars sync` so the item is immediately installed back through the normal pipeline
- Same-filesystem only in MVP; the target directory must be on the same filesystem as the project root

`mars adopt` works in any project — `[package]` in `mars.toml` is not required.

```bash
mars adopt .agents/agents/my-agent.md        # adopt an agent
mars adopt .agents/skills/my-skill           # adopt a skill
mars adopt .agents/agents/my-agent.md --dry-run  # preview only
```

After adoption, the item lives in `.mars-src/` (your editable source) and is tracked in `mars.lock`. Edit it there and run `mars sync` to propagate changes.

---

## `mars resolve`

Mark conflicts as resolved after manually editing conflicted files.

```bash
mars resolve [file]
```

| Argument | Description |
|---|---|
| `file` | Specific file to resolve (default: all conflicted items) |

Checks for remaining conflict markers (`<<<<<<<`, `>>>>>>>`). If the file is clean, updates the lock file's `installed_checksum` to match the current disk content.

> **Note:** Current sync behavior is source-wins — `mars sync` overwrites local modifications with upstream content (with a warning) rather than producing conflict markers. Conflict markers would only appear from manual edits to managed files or legacy state from an older mars version. `mars resolve` remains available to clear them when they do exist.

---

## `mars override`

Set a local development override for a source.

```bash
mars override <name> --path <local-path>
```

Writes to `mars.local.toml` and re-syncs. The local path replaces the git URL for resolution. `<name>` is the dependency name from `mars.toml`. See [local-development.md](../dev/local-development.md).

---

## `mars link`

Add a managed target directory.

```bash
mars link <target> [--force]
```

| Flag | Description |
|---|---|
| `--force` | Adopt collisions without an installed-content claim (overwrite + record in lock) |

**Behavior:**
- Adds `<target>` as a managed target directory and copies content from `.mars/` into it
- Persists the target in `mars.toml [settings] targets` and updates `mars.lock` with per-target output records
- If a path in the target exists on disk without an installed-content claim for that `target_root` (no output record, or a pending-deletion record), Mars preserves it and emits `target-unmanaged-collision` (exit 2). Run `mars link <target> --force` to adopt and record installed ownership (`target-unmanaged-adopted`)
- Materialization depends on target, `agent_emission`, and optional `[settings.meridian.agent_copy]` (e.g. `.cursor` receives skills/MCP unless native agent emission or selective copy applies)

```bash
mars link .claude            # Copy managed content into .claude/
mars link .cursor            # Copy skills/MCP into .cursor
mars link .claude --force    # Adopt collisions without an installed-content claim in .claude/
```

---

## `mars unlink`

Stop managing a target and remove Mars-owned content from it.

```bash
mars unlink <target>
```

**Behavior:**
- Removes `<target>` from `mars.toml [settings] targets` (and `managed_root` if it matches)
- Removes only outputs and config entries owned by the target's lock records
- Removes empty ancestor directories, including the target directory when it becomes empty; preserves handwritten or otherwise unowned content
- Reports if the target was not managed (no-op)

```bash
mars unlink .agents          # Remove deprecated .agents target
mars unlink .claude          # Stop managing .claude
```

---

## `mars models`

Model identity, catalog metadata, and the human harness-model inventory are
separate commands:

```bash
mars models refresh
mars models list [--all] [--live] [--harness NAME] [--match GLOB] [--refresh-models|--no-refresh-models]
mars models aliases [--refresh-models|--no-refresh-models]
mars models catalog [--refresh-models|--no-refresh-models]
```

`mars models refresh` fetches models.dev metadata synchronously into
`.mars/models-cache.json`. The catalog is indefinite last-known-good data:
the default 24-hour interval controls when to *refresh*, not when reads expire.
Fresh cache returns without network work. Stale usable cache returns immediately
and claims one detached background refresh across concurrent readers; cold/empty/corrupt cache fetches
synchronously. Failed or empty refreshes retain last-good data and back off for
five minutes. `--refresh-models` forces a synchronous fetch;
`--no-refresh-models` and `MARS_OFFLINE` use disk only, with no worker or probe.
`MARS_OFFLINE` also overrides `--refresh-models`. The internal
`models __refresh-catalog` worker is not a user-facing command. Stale JSON
`cache_refresh.refresh.status` distinguishes `spawned`, `already_in_progress`,
`cooldown`, and `spawn_failed`; none reports that the asynchronous fetch succeeded.
If a peer completes between the initial read and the launch claim,
`cache_refresh.status = "peer_refreshed"` returns its new cache snapshot without
launching another worker, including with refresh-after set to zero. A failed
peer instead yields `cooldown` with its recorded failure. The refresh claim
expires after 120 seconds if a worker crashes; the models.dev HTTP request has
a 60-second end-to-end deadline covering DNS, redirects, and response body,
leaving room for startup and cache writes. Ad-hoc
`build launch-bundle` can refresh its cache without a `mars.toml` project.

### `mars models list`

Lists curated harness×model rows from installed-harness Possible evidence plus
literal declarations. The default view shows effective `show` rows; `--all`
includes hidden rows and their decision tier. `--harness` accepts a registered
harness name; `--match` is an ad-hoc case-insensitive glob over the exact launch
model ID (`*` crosses `/`). Neither narrowing option changes curation or routing.
`--live` adds fixed-harness routing eligibility (`eligible`, `unverified`, or
`blocked`) and its reason; it never substitutes a different provider's model.
Probe-backed rows use retained listing evidence. A declared row for an
uninstalled harness is shown with `blocked/not_installed` under `--live`.

Text columns are HARNESS, MODEL (exact harness launch ID), PROVIDER, ORIGIN
(`possible`, `both`, or `declared`), VIA (`catalog`, `listed <age>`,
`listed <age>, refresh failed`, or `—`),
CURATION, ELIGIBILITY, and ALIASES. `--json` returns `models` rows with
`harness`, `harness_model_id`, `model_id`, `provider`, `origin`, full `provenance`,
`via`, `aliases`, `curated: {decision,tier}`, and `eligibility`/`reason`.
`eligibility` and `reason` are always present: both are `null` without
`--live`; with `--live`, eligibility is a string or `null` if no assessment
can be associated with that exact row, and reason is a string or `null`.
`provider` and `provenance` may be `null`; enumerated provenance
includes `probe`, `observed_at`, `auth_gated`, `latest_attempt_ok`, and
`last_error`. Harness and probe IDs use registry names, including `opencode`.
Top-level `diagnostics`, `routing_diagnostics`, `cache_warning`, and
`cache_refresh` report
problems without dropping rows. An installed in-scope harness without a
retained listing appears in `diagnostics`; a failed attempt includes its last
error. A retained listing with a failed latest refresh remains visible in row
provenance and also emits one top-level diagnostic per harness, even if every
row is hidden by curation.
`cache_refresh.status = "stale"` identifies last-good catalog data;
`cache_refresh.refresh.status` is `spawned`, `already_in_progress`, `cooldown`, or `spawn_failed`.
`peer_refreshed` reports that another worker completed during this read; it
does not start a replacement worker.
`spawned` means only that a worker started, not that its fetch succeeded.
`last_failure` records the prior failed fetch when present.

### `mars models aliases`

Lists statically resolved consumer, dependency, and built-in model aliases.
It never applies curation or probes harnesses. `--json` returns `aliases`
with `name`, `model_id`, `provider`, `harness`, `harness_candidates`, `mode`,
and alias metadata. `harness` is nullable static authored preference;
`harness_source` is not emitted because no route has been assessed. Meridian's
alias inventory should use this command.

### `mars models catalog`

Lists raw models.dev cache entries, independent of curation, installed
harnesses, and harness scope. It never probes harnesses. `--json` returns
`catalog` with each entry's `id`, `provider`, `description`, `release_date`,
`context_window`, `max_output`, and `cost_input`, `cost_output`,
`cost_cache_read`, `cost_cache_write`, `cost_reasoning` fields (optional
metadata and numeric fields are nullable). Meridian's exact-ID guard and
catalog sync should use this command. The old list-all `harness`,
`harness_candidates`, `matched_aliases`, `runnable_paths`, and `availability`
fields do not exist here; P4 must derive or drop them.

**Breaking release contract:** Meridian P4 changes `models list --json` to
`models aliases --json` and `models list --all --json` to
`models catalog --json`. Publish Mars first; existing Meridian releases remain
on their exact older `mars-agents` pin. After the new Mars package is available,
update Meridian's exact pin and lock, verify the installed binary, then release
Meridian. An installation that overrides Meridian's bundled Mars through `PATH`
is not protected by the dependency pin and must coordinate both upgrades.

`--include`, `--exclude`, `--providers`, `--no-visibility`, `--catalog`, and
`--unavailable` were removed from `models list`. Put display rules in
[`mars.curated.toml`](../config/mars-curated.md). Legacy
`[settings.model_visibility]` in `mars.toml` or `mars.local.toml` is a targeted
error translating the file's actual values; Mars does not rewrite config
automatically. A dependency package's consumer-only settings are ignored by
manifest loading and never require the consumer to migrate upstream files.

### `mars models prompting`

Print the prompting guidance for an alias or an Agent.

```bash
mars models prompting <reference> [--json]
```

A bare reference (`gpt55`) is a model alias and prints the alias's `prompting` text. `@name` is an Agent: Mars prints the guide the Agent's own `model-policies` rules declare, one line for each rule that has a `when`, ahead of the alias's text.

```text
$ mars models prompting @coder
Model guide for coder:
- model-glob `gpt-*`: for small edits
```

Each line is the rule's `match` kind and value, then its `when` text. `--json` returns `ref`, `ref_kind` (`agent` or `alias`), `agent_name`, `model_alias`, `model_name`, `found` and `prompting`. An unknown reference exits 1. The Agent inventory in a launch bundle marks Agents that have a guide with `mars models prompting @name`. See [`when`](../config/schema-v1.md#model-policies).

### `mars models resolve`

Show the resolution chain for one alias.

```bash
mars models resolve <alias>
```

```bash
mars models resolve opus
```

### `mars models alias`

Quick-add a pinned alias to `mars.toml [models]`.

```bash
mars models alias <name> <model-id> [--harness HARNESS] [--description TEXT]
```

| Argument/Flag | Description |
|---|---|
| `name` | Alias name to create |
| `model-id` | Concrete model ID to pin |
| `--harness` | Harness name (default: `claude`) |
| `--description` | Optional human-readable description |

```bash
mars models alias opus claude-opus-4-6
mars models alias fast gpt-5.3-codex --harness codex --description "Fast coding model"
```

---

## `mars check`

Validate a source package before publishing.

```bash
mars check [path]
mars check --root PATH
```

| Argument | Description |
|---|---|
| `path` | Directory to validate (default: current directory) |
| `--root PATH` | The same directory, named as every other command names its root; giving it with `path` is an error |

Does not require a mars project (no `mars.toml` needed). Validates:
- `requires-mars` and `requires-meridian` syntax when `[package]` is present
- Package structure: convention folders named `agents/`, `skills/`, or `bootstrap/` at non-hidden depth up to `MAX_DISCOVERY_WALK_DEPTH = 5`, plus package-root `SKILL.md` fallback
- Prints the `source package validates` line only when there are no errors
- Definitions against [schema v1](../config/schema-v1.md): the same readings and diagnostics `mars sync` applies, so removed keys, invalid values and malformed YAML are errors (exit 1), unknown keys warn, and a package with no `schema` is read as pre-v1 with a warning that names `mars migrate`
- Frontmatter: name, description presence and consistency
- Duplicate `(kind, name)` discoveries within the package
- Skill dependency references (warns about external deps)
- Symlinks in source packages (warned as unsupported)
- Missing `SKILL.md` in skill directories
- Configured plugin output: missing/skipped builds, stale inputs, a different package or Mars
  version, hand-edited or missing generated files, root files left holding an earlier build's
  output, and conflicts at generated package-root paths
- `[package].requires-mars` floor: `">=0.20.0"` (or higher) for a package that declares
  `schema = 1`, the first Mars that reads it; `">=0.19.0"` for a plugin package with no
  `schema`, the first that keeps `[plugin]`
- Plugin compatibility: unknown `[plugin].harnesses` values

If a `mars.toml` is present in the target directory, also runs a **field-loss
preview** (`collect_prepared_source_lossiness_diagnostics`) against the configured managed
targets — reports what each native target cannot express from the source tree's
agents and skills.

```bash
mars check                   # Check current directory
mars check ../my-agents      # Check another directory
```

---

## `mars version`

Version and release a package repository.

```bash
mars version <patch|minor|major|X.Y.Z> [--push] [--force] [--no-plugins]
```

The command requires a clean Git worktree, runs the package publish checks, updates
`[package].version` and `CHANGELOG.md`, commits the release, and creates the annotated version
tag. `--push` pushes the current branch and tag after the local release succeeds.

When `[plugin]` is configured, `mars version` first stages all selected plugin trees at the new
version. It then publishes the plugin directory and generated root catalogs/`package.json` in the
same release commit. Build or commit failure restores every release-touched path and the starting
commit. `--no-plugins` skips this configured plugin build; use it only when intentionally
releasing with unchanged plugin output. Plugin configuration is still validated.

---

## `mars doctor`

Diagnose problems in an installed mars project.

```bash
mars doctor
```

Checks:
- Config validity (parses `mars.toml`)
- Lock file integrity
- Each locked item: file exists on disk, no conflict markers, checksum computability
- Config-lock consistency: dependencies in config match lock entries
- Agent skill references: every declared skill dependency exists on disk
- Target health: each managed target has the expected files with correct content
- Target divergence: detects missing and locally modified files in targets, suggests `mars sync --force` or `mars repair`

Exit code 0 = healthy, 2 = issues found. See [troubleshooting.md](../dev/troubleshooting.md).

---

## `mars repair`

Rebuild managed state from config + dependencies.

```bash
mars repair
```

Runs one forced sync. If the lock file is corrupt, repair treats it as empty in
memory and rebuilds from config plus dependencies. The corrupt on-disk bytes
are replaced only when the full pipeline finalizes successfully. If recovery
encounters unreadable removed-schema hooks, it halts before materialization
and leaves the corrupt lock untouched.

---

## `mars cache`

Manage the global source cache.

```bash
mars cache <info|clean>
```

### `mars cache info`

Show cache location and disk usage (total, archives, git clone cache).

- Prints cache path plus total size breakdown.
- Supports `--json` with `path`, `total_bytes`, `archives_bytes`, and `git_bytes`.

### `mars cache clean`

Remove cached source trees (archives + git clones).

- Removes cache contents while preserving the cache directory structure.
- Prints reclaimed bytes (total, archives, git) and supports `--json`.

---

## `mars build`

Build artifacts from static project state.

```bash
mars build <launch-bundle|plugin> ...
```

### `mars build plugin`

Build installable plugin trees from a publishable package and the complete graph that `mars sync`
installs in that package's own checkout: its items plus dependencies under its own
`[dependencies]` filters, renames, and visibility rules.

```bash
mars build plugin [PATH] [--out <dir>]
```

`PATH` defaults to the detected project root. With an explicit path the command can run outside
a Mars project. The build uses only `mars.toml` publish state: it does not read `mars.local.toml`,
`mars.lock`, `.mars/`, project target configuration, or installed harness model catalogs.

Without `--out`, `[plugin]` is required and the command writes `[plugin].dir` (default `plugin`).
The configured build also writes the Claude and Cursor marketplace catalogs plus the root OpenCode
`package.json`. With `--out`, Mars writes only the output directory and accepts a package without
`[plugin]`; an explicit destination cannot resolve to the configured directory.

The output is an owned nested artifact:

```text
<dir>/
  claude/    # Claude, Codex, and Grok
  cursor/    # Cursor
  opencode/  # OpenCode
  .mars-plugin.json
```

Only trees selected by `[plugin].harnesses` are written. The shared Claude tree contains native
agents, skills, exported hooks, exported MCP servers, and `.claude-plugin/plugin.json`; Codex
loads its skills, hooks, and MCP servers but not its agents. Cursor receives its native tree.
OpenCode receives an npm-shaped package that registers skills and MCP servers and composes
default-export hook fragments.

Mars records generated file hashes, input and dependency identities, version, and root-file
ownership in the sentinel. It atomically replaces owned output, recovers interrupted staging, and
refuses a non-empty directory or root file it does not own. See
[Authoring harness plugins](../plugin-authoring.md).

### `mars build launch-bundle`

Build a harness-targeted launch bundle — a JSON artifact containing routing, execution policy, prompt surface, and tooling metadata for spawning an agent.

```bash
mars build launch-bundle [--agent NAME] [--model TOKEN] [flags]
```

**Two modes:**

**Ad-hoc mode** (no `--agent`):
- Requires `--model <token>`
- Does **not** require a `mars.toml` project — runs from any plain directory
- Uses no agent profile; routing is driven by `--model` and the installed harness environment

**Agent/profile mode** (`--agent <name>`):
- Reads `.mars/agents/<name>.md` from the nearest `mars.toml` project
- Requires a project with `mars sync` already run
- Inherits the profile's fields (model, harness, effort, approval, sandbox, skills, tools, model-policies)
- Model precedence is `--model` > the `[agents.<name>]` overlay > profile model > `settings.default_model`.
- Effort, approval and sandbox follow the [precedence chain](../config/schema-v1.md#model-policies): CLI flag, overlay field, the first matching `model-policies` rule, the profile's own value, then the alias's `default_effort`.

| Flag | Description |
|---|---|
| `--agent <name>` | Agent name from `.mars/agents/<name>.md`. Omit for ad-hoc mode. |
| `--model <token>` | Model token or canonical model ID. Required in ad-hoc mode; overrides profile in agent mode. |
| `--harness <name>` | Override harness target (`claude`, `codex`, `opencode`, `cursor`, `pi`). |
| `--effort <level>` | Override effort: `none`, `low`, `medium`, `high`, `xhigh`. `none` asks for no reasoning effort and is written as `"effort": "none"` in the bundle; see [Effort](../config/schema-v1.md#effort) for what each harness does with it. |
| `--approval <mode>` | Override approval: `default`, `auto`, `confirm`, `never`. |
| `--sandbox <mode>` | Override sandbox: `default`, `read-only`, `workspace-write`, `danger-full-access`. |
| `--skill <name>` | Add extra skills by name. Repeatable; also accepts `--skill a,b`. |

**Output JSON structure:**

```jsonc
{
  "version": 4,
  "agent": "agent-name-or-null",
  "agent_source_path": "...",
  "agent_body": "raw-agent-markdown-body",
  "routing": {
    "model": "...",
    "harness": "...",
    "selection_kind": "auto|fixed",
    "match_evidence": "confirmed|constrained|passthrough|none",
    "harness_model": "...",
    "harness_model_source": "provider-match|cached-probe|passthrough|synthesized",
    "harness_model_confidence": "confirmed|likely|unknown",
    "route_trace": { "version": 3, "model_attempts": [{ "selected_by_preference": false }], "..." }
  },
  "execution_policy": {
    "effort": "high",
    "approval": "auto",
    "sandbox": "workspace-write",
    "autocompact": null,
    "autocompact_pct": null,
    "timeout": null
  },
  "prompt_surface": { "..." },
  "scaffold_slots": { "..." },
  "tools": {
    "capability_natives": {
      "agent": { "native": ["Agent", "SendMessage", "ListAgents"], "native_status": "known" },
      "task": { "native": ["TaskCreate", "TaskGet", "TaskList", "TaskOutput", "TaskUpdate", "TaskStop", "TodoWrite"], "native_status": "known" }
    },
    "rules": [
      { "capability": "edit", "pattern": null, "action": "deny",
        "native": ["Edit", "Write", "NotebookEdit"],
        "native_tools": ["Edit", "Write", "NotebookEdit"], "native_status": "known" },
      { "capability": "bash", "pattern": "rg *", "action": "allow",
        "native": ["Bash(rg *)"], "native_tools": ["Bash"], "native_status": "known" }
    ],
    "allowed": ["Bash(rg *)"],
    "disallowed": ["Edit", "Write", "NotebookEdit"],
    "mcp": []
  },
  "skills": {
    "loaded": [{ "name": "...", "skill_type": "...", "source_path": "...", "body": "..." }],
    "available": [{ "name": "...", "skill_type": "...", "description": "...", "source_path": "..." }],
    "missing": ["..."]
  },
  "provenance": {
    "model_source": "cli|profile|project",
    "harness_source": "...",
    "effort_source": "cli|overlay|overlay-model-policy|profile-model-policy|profile|settings-model-policy|alias|unset",
    "approval_source": "...",
    "sandbox_source": "...",
    "autocompact_source": "...",
    "autocompact_pct_source": "...",
    "matched_policy_rule": "profile:0",
    "selection_kind": "...",
    "match_evidence": "...",
    "candidates_tried": "..."
  },
  "warnings": ["..."]
}
```

- `agent_body` is the raw post-frontmatter markdown body from `.mars/agents/<name>.md` in agent/profile mode.
- `agent_body` is omitted in ad-hoc mode (no `--agent`).
- `skills.*.skill_type` is the skill's `metadata.type`, or `reference` when it sets none.
- `execution_policy` holds `effort`, `approval`, `sandbox`, `autocompact`, `autocompact_pct` and `timeout`. Compaction comes only from the model alias; an Agent has no compaction or network setting. `effort` is `none` when `--effort none` or a rule set it.
- Each `provenance.*_source` names the step of the precedence chain that supplied the value, and `matched_policy_rule` names the one `model-policies` rule that matched the chosen model (`profile:<index>`, `overlay:<index>` or `settings:<index>`), even when a higher step then set the field; it is absent when no rule matched.

**`tools.capability_natives`** is always present, independent of the profile's tool policy. It
maps the `agent` (sub-agent) and `task` (task-list) capabilities to the resolved harness's
native tools so a launcher can enforce platform rules without maintaining its own name table.
Each entry has `native` and `native_status` with the same meaning and projection source as a
`tools.rules[]` entry, plus `note` when the status is `unknown`. `absent` means the harness has
no such tool. The native list deliberately includes names retained for supported older harness
builds, such as Claude's `TaskOutput`.

**`tools.rules`** is the tool-policy contract: one entry per statement in the profile, in
profile order, for the resolved harness. Read meaning from `capability` and emission from
`native`, and tool availability from `native_tools`; do not derive any one from another.

| Field | Meaning |
|---|---|
| `capability` | Canonical capability (`bash`, `edit`, `web`, …), `*` for "every other tool" (an allowlist; it names no tool, so `native` and `native_tools` are empty and `native_status` is omitted), `mcp` for MCP refs, or the author's spelling when Mars does not recognize the name |
| `pattern` | Scope inside `capability(...)`, or `null`. A `bash` prefix `X:*` is normalized to `X *`. For `mcp`, the `server/tool` reference |
| `action` | `allow`, `deny` or `ask` |
| `native` | The harness's permission names for the rule, after scoped-name and conflict resolution. Empty unless `native_status` is `known`; a known rule can also be empty when a stricter or more specific rule owns the shared permission. Already resolved: `edit: deny` + `write: allow` on Claude gives the `edit` deny `Edit`, `NotebookEdit` and leaves `Write` allowed, so emit `native` as given |
| `native_tools` | The unscoped harness tool names concerned by the surviving `native` permissions. Never strip patterns or otherwise derive this field from `native`. For a `*: deny` allowlist, the availability set (such as Claude `--tools`) is the union from known `allow` and `ask` rules; a `deny` rule never grants availability. Empty whenever `native` is empty |
| `native_status` | `known`: Mars knows the harness permission, whether it survives conflict resolution or is shadowed. `absent`: the harness has no such tool, nothing to enforce. `unknown`: Mars cannot name it for this harness, a gap to report. Omitted for `*` |
| `note` | A harness-specific consequence the consumer must surface. On an `unknown` rule, why Mars could not name it (`"codex loads skills by reading files; no switch"`); on a `known` rule, why the valid native permission is broader, why a stricter rule shadowed it, or why the harness must decide an unresolved overlap. Permission gaps never add an entry to `warnings[]`: `note` is the only channel |

`tools.allowed`, `tools.disallowed` and `tools.mcp` are flat native-name lists derived from
`rules` for consumers that predate it. They cannot express `ask`, `*`, or the capability behind
a name; `allowed` never reflects an allowlist, and `mcp` never contains server definitions or
commands. Prefer `rules`. A tool that an allow and a deny
rule both still name (`write: allow` with `edit: deny` on Codex, whose `apply_patch` is both)
puts a `note` on both rules.

Claude path scopes use its permission categories, not every bare tool name:
`write(path)`, `edit(path)` and `notebook(path)` become `Edit(path)`, while `read(path)`,
`glob(path)` and `grep(path)` become `Read(path)`. If two scoped capabilities become the same
permission, Mars keeps the strictest action (`deny`, then `ask`, then `allow`) and notes the
shadowed rule. `native_tools` separately keeps the callable tools: scoped `write` names
`["Write"]`, scoped `edit` names `["Edit", "Write", "NotebookEdit"]`, and scoped `grep`
names `["Grep"]`. `WebFetch(domain:...)` is already the native scoped form.

The two fields stay distinct on every harness:

| Harness | Canonical rule | `native` permission | `native_tools` availability |
|---|---|---|---|
| Claude | `write(src/**)` | `Edit(src/**)` | `Write` |
| Codex | `bash(git *)` | `exec_command(git *)` | `exec_command` |
| OpenCode | `web` | `websearch`, `webfetch` | `websearch`, `webfetch` |
| Pi | `edit(src/**)` | `edit(src/**)`, `write(src/**)` | `edit`, `write` |
| Cursor | `notebook(file.ipynb)` | `EditNotebook(file.ipynb)` | `EditNotebook` |

**Warning semantics:** `warnings[]` contains only unexpected, user-actionable conditions. Routing path facts are NOT warnings — `harness_model_source: "passthrough"` and `harness_model_confidence: "unknown"` (e.g., Pi or explicit harness) appear in routing/provenance fields and do not produce warnings. Real warnings include: enabled-target constraints exhausting auto-routing candidates.

**`harness_model` resolution:** Alias `provider` does not always become `provider/model` in
`routing.harness_model`. Native Codex/Claude preserve the requested spelling;
Pi/OpenCode use the selected probe slug. Example: `-m gptmini` on a project with that alias → Codex +
`harness_model: "gpt-5.4-mini"` (`provider-match`), not `openai/gpt-5.4-mini`. See
[`src/models/.context/CONTEXT.md`](../../src/models/.context/CONTEXT.md).

```bash
# Ad-hoc: route gpt-5.4-mini from any directory
mars build launch-bundle --model gpt-5.4-mini --json

# Agent mode: build bundle for a synced agent
mars build launch-bundle --agent my-agent --json

# Override harness explicitly
mars build launch-bundle --model gpt-5.4-mini --harness codex --json

# Add extra skills
mars build launch-bundle --agent my-agent --skill my-skill --json
```
