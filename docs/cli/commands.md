# CLI Reference

All commands support two global flags:

| Flag | Description |
|---|---|
| `--root <PATH>` | Explicit project root (default: auto-detect from cwd) |
| `--json` | Machine-readable JSON output |

The project root is the directory holding `mars.toml` and `mars.lock`.

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | `check`: validation errors found; `resolve` (deprecated): unresolved conflict markers remain |
| `2` | Config, resolution, validation, lock, request, collision, or frozen-violation errors, including a definition that breaks [schema v1](../config/schema-v1.md) |
| `3` | I/O, source fetch, HTTP, or git CLI errors |

---

## `mars init`

Initialize a mars project. Creates `mars.toml` and `.mars/`.

```bash
mars init [TARGET] [--link DIR...]
```

| Argument/Flag | Description |
|---|---|
| `TARGET` | Optional managed output directory to create (default: none) |
| `--link DIR` | Directories to link after init (repeatable) |

**Behavior:**
- Creates `<project-root>/mars.toml` with `[dependencies]` section
- Creates `.mars/` internal state directory
- Creates `TARGET`, when given, and adds it to `settings.targets`
- Adds `mars.local.toml` and `.mars/` to `.gitignore`
- Idempotent: re-running is a no-op for init, but still processes `--link`
- When the project's `mars.toml` declares `[package]`, runs a **field-loss preview**
  against configured managed targets, reporting what each target cannot express
  (lossiness diagnostics via `collect_prepared_source_lossiness_diagnostics`). A consumer
  project has no package to publish, so it gets no preview

```bash
mars init                    # No target directory; link one with `mars link`
mars init .claude            # Create .claude/ as the single managed target
mars init --link .claude     # No extra directory; add `.claude` to settings.targets
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
- A dependency whose definitions break schema v1 is rejected before any content is written: the command exits 2 and `mars.toml` does not gain the dependency (see [sync](#mars-sync)).
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

Resolve dependencies, install into `.mars/`, and copy managed content into each target.

```bash
mars sync [--force] [--diff] [--frozen]
```

| Flag | Description |
|---|---|
| `--force` | Overwrite local modifications for managed files |
| `--diff` | Dry run: show what would change without applying |
| `--frozen` | Error if lock file would change (CI mode) |
| `--verbose` | Also print `info` notes, such as a key in a dependency that Mars does not read, and every lossy field per item and target |

This is the core operation. It runs the full [sync pipeline](../internals/sync-pipeline.md): resolve → prepare → target → diff → apply. A project whose `min_mars_version` the running Mars does not meet fails with `[compat-version]` and exit 2 before anything is resolved or written. Mars reads every dependency's and the project's own definitions as [schema v1](../config/schema-v1.md) before it writes any installed content. An error (`field.value`, `field.type`, `field.required`, `tools.list-form`, `yaml.*` and `frontmatter.*`) fails the sync with exit 2 and changes no installed content, target file or `mars.lock` (a fresh project still gains `.mars/sync.lock`, created before the gate). A dependency's `hook.toml` that fails validation (a removed `fragment` key, say) does not fail the sync: Mars warns, naming the package, and keeps the hook as the last sync installed it. Warnings (a restriction a translation could not keep, the lossiness a target cannot express) are printed and the sync goes on. A key or table Mars does not read in the project's own `mars.toml` or `mars.local.toml` (`field.unknown`) is a warning that names the closest key it does read (`target` → `targets`, `[setting]` → `[settings]`), because a misspelt `targets` leaves no targets and the sync removes what Mars installed in them. In a dependency or the user config it is an `info` note: `--verbose` prints it, a default sync does not. A dependency whose `[package]` has no `schema` is refused (`manifest.schema-missing`). A key Mars no longer reads is a `field.retired` warning that links to [Retired keys](../config/retired-keys.md). Native agent materialization follows `settings.agent_emission`; `[settings.meridian.agent_copy]` can selectively emit qualifying harness-native agent copies even when emission is otherwise suppressed.

A target that no `mars.toml` or `mars.local.toml` names any longer has what Mars installed in it removed by the next sync, as `mars unlink` does. An output that no longer matches what Mars wrote (an edited skill, a file you added inside a skill's directory) stays, loses its record, and the sync warns once per target (`output-kept`), so a second sync is quiet. A path that cannot be deleted (`output-remove-failed`) keeps its record and does not stop the sync; the next one retries it, and until it succeeds the summary says `pending` for that target instead of `removed` (`pending_targets` in `--json`). An empty directory Mars cannot remove is left without a warning. `--diff` lists what it would delete and, with the same warning, what it would keep.

The summary lists each generated hook file Mars wrote (`mars-<name>.hooks.ts`, a native hook written to a file target) as `~ <target>/<path> (generated hook file)` and counts them as `wrote N generated hook files`; a run that writes only those is not "already up to date". `--diff` previews them as `would write N generated hook files`. `--json` carries the count as `hook_files_rewritten`.

The summary also lists each MCP config file a run wrote or deleted (`.mcp.json`, `.codex/config.toml`, `.opencode/opencode.json`, `.pi/mcp.json`) as `~ <file> (MCP servers, <target>)`; the other files Mars edits by key in the same way (an `agy` plugin's `.agents/plugins.json`, `.agy/plugin.json` and `.agy/hooks.json`) are listed with them, named by what they hold (`plugin entries`, `plugin manifest keys`, `hooks`). All of them count as `updated N config files`, so adding an MCP-only package or a project-root `mcp/` entry is not reported as "already up to date". `--json` carries the MCP files' count as `mcp_files_changed`; the other keyed files are not in it. `--diff` previews them as `would update N config files`. `already up to date` prints when the sync changed none of the installed items, hook files and MCP files above and raised no warning. A native agent file the sync recreates because you deleted it is not counted, so that sync can print the line too. The exceptions are advisory warnings, which say nothing about whether the sync reached its state: lossiness notes, unset env vars, unknown hook events, skipped or renamed names, and package format or version notices. Any other warning, such as a failed write or an entry Mars left in place because it was edited by hand, withholds the line. A sync that rewrites a hand-edited hook file, native agent or skill copy also warns, so it does not print the line either. `--diff` does not look at every file a real sync rewrites, so with nothing pending it prints `no pending changes found`, not `already up to date`. `--diff` also shows the MCP warnings a real sync would, such as a hand-edited server it would leave alone; `mars validate` does not.

---

## `mars upgrade`

Upgrade dependencies to newest versions within their constraints.

```bash
mars upgrade [names...]
```

| Argument | Description |
|---|---|
| `names` | Specific dependency names to upgrade (default: all). Any dependency in the resolved graph, one a package requires as well as one `mars.toml` names |

Takes the newest compatible version instead of keeping the locked one. `mars sync` never does this, for a dependency with no `version` either: `mars.lock` is kept until you run `mars upgrade`. Git sources are upgradeable, with or without a `version`; path sources have no version to maximize. An untagged repository's dependency locked at a commit moves to the first tag it gains, or to the new HEAD. The summary names each dependency it moved (`moved       up v1.0.0 → v1.1.0`, or `up 1a2b3c4d5e6f → v1.0.0` from a commit) even when no installed file changed; `--json` carries them as `dependencies_moved` (`name`, `from`, `to`, `transitive`). It moves dependency versions and never edits definitions.

A marketplace plugin is refreshed as a whole: Mars re-reads the marketplace at its ref, takes the plugin's current source from it and resolves the plugin's content commit anew, even when the marketplace itself did not change. A plugin whose source moved to another repository or path is reported (`plugin <name> of <dependency> moved: <old> -> <new>`).

```bash
mars upgrade                 # Upgrade all
mars upgrade base dev        # Upgrade specific deps
```

---

## `mars outdated`

Show available updates without applying them.

```bash
mars outdated
```

Displays a table with columns: SOURCE, LOCKED, CONSTRAINT, UPDATEABLE, LATEST. It lists every dependency in the resolved graph, the ones `mars.toml` names first, then the ones a package requires, marked `(transitive)` (`"transitive": true` in `--json`); their CONSTRAINT is what the requiring packages ask. `mars upgrade <name>` takes any name listed here, and the hint `mars sync` prints (`ℹ 2 upgrades available`, one `name v1.0.0 → v1.1.0` line each, and `upgrades` in `--json`) names the same dependencies.

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

## `mars agents` and `mars skills`

Inspect the installed agents or skills, after variants and overrides are applied.

```bash
mars agents [list] [--mode MODE] [--model-invocable] [--source NAME] [--fallback-root PATH]
mars agents show NAME
mars skills [list] [--type TYPE] [--model-invocable] [--source NAME] [--fallback-root PATH]
mars skills show NAME
```

| Flag | Description |
|---|---|
| `--mode MODE` | Agents only: `primary` or `subagent` |
| `--type TYPE` | Skills only: match `metadata.type` exactly |
| `--model-invocable` | Only items a model may pick for itself |
| `--source NAME` | Filter by dependency name |
| `--fallback-root PATH` | A secondary Mars project consulted only for items the project lacks |

The table shows name, mode or type, `M-INV` (model-invocable), scope (`root` or `fallback`) and description. `--json` prints `{"agents": [...]}` or `{"skills": [...]}`; each entry has `name`, `description`, `mode` (agents; `subagent` when the profile omits it, and `--mode subagent` matches such agents) or `type` (skills), `model-invocable`, `scope` and `source_path`. Agent entries add three fields:

- `native-harnesses`: the harnesses the last `mars sync` installed the agent into natively.
- `fanout`: `true` when `[settings.meridian.fanout].agents` names it, so Meridian launches it even where it is native.
- `subagents`: the profile's list of agents it may spawn.

`show` prints the same entry for one item, plus `user-invocable` and, for agents, the full launch profile. Meridian's `meridian spawn subagents` reads `mars agents --json`.

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

A relative `path` resolves against the working directory when it exists there; otherwise, with an explicit `--root`, against the project root it selects.

`mars adopt` works in any project — `[package]` in `mars.toml` is not required.

```bash
mars adopt .claude/agents/my-agent.md        # adopt an agent
mars adopt .claude/skills/my-skill           # adopt a skill
mars adopt .claude/agents/my-agent.md --dry-run  # preview only
```

After adoption, the item lives in `.mars-src/` (your editable source) and is tracked in `mars.lock`. Edit it there and run `mars sync` to propagate changes.

---

## `mars resolve`

> **Deprecated** in 0.39.0 and removed in the next minor release. It warns (`command.deprecated`) each run. See [what changed](../config/retired-keys.md#mars-resolve).

Mark conflicts as resolved after manually editing conflicted files.

```bash
mars resolve [file]
```

| Argument | Description |
|---|---|
| `file` | Specific file to resolve (default: all conflicted items) |

Checks for remaining conflict markers (`<<<<<<<`, `>>>>>>>`). If the file is clean, updates the lock file's `installed_checksum` to match the current disk content.

> **Note:** Current sync behavior is source-wins — `mars sync` overwrites local modifications with upstream content (with a warning) rather than producing conflict markers. Conflict markers would only appear from manual edits to managed files or legacy state from an older mars version. `mars resolve` still clears them until it is removed; to keep a change to an installed item, use an overlay or edit the source instead.

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
mars link <target> [--force] [--local]
```

| Flag | Description |
|---|---|
| `--force` | Same as `mars sync --force`: adopt colliding paths in every target, not only this one, and reset local edits to managed items under `.mars/` |
| `--local` | Add the target to `mars.local.toml` instead of `mars.toml`: only this machine syncs it, and its lock records go to `.mars/machine.lock`, so `mars.toml` and the committed `mars.lock` do not change |

**Behavior:**
- Adds `<target>` to `mars.toml [settings] targets` and runs a sync, so the target gets everything a sync installs there: skills, agents, hooks, MCP servers and their config entries. Linking a target `mars.local.toml` names moves it: it leaves `mars.local.toml`, and its records move from `.mars/machine.lock` to `mars.lock`
- With `--local`, a target `mars.toml` already names stays a project target, nothing is written, and the command says so; otherwise the target is added to `mars.local.toml` and `min_mars_version = "0.39.0"` is written there unless the project's effective floor is already that high, so Mars 0.38.0 stops with `compat-version` instead of misreading the list; its records go to `.mars/machine.lock` ([lock files](../internals/lock-file.md#two-files))
- If a path in the target exists on disk without an installed-content claim for that `target_root` (no output record, or a pending-deletion record), Mars preserves it and warns `target-unmanaged-collision`, as a sync does. `mars link <target> --force` is `mars sync --force`: it adopts colliding paths in every target, not only this one, and resets local edits to managed items under `.mars/`, which a plain sync keeps with a `disk-lock-divergent` warning
- A harness target is named by its directory (`.agy`) or its harness (`agy`); either form is saved as the directory and reads back as the harness. `.agy` is not a copy of `.mars/` but an `agy` plugin: skills, agents, hooks and MCP servers under `.agy/`, plus `.agents/plugins.json` (a `{"path": ".agy"}` entry, appended after the user's) and `.agy/plugin.json`. See [Antigravity](../config/mcp-and-hooks.md#antigravity-agy). Linking `.agents` beside `.agy` warns, because `agy` reads `.agents/skills/` too and sees each skill twice.
- `mars link cursor` fails: Mars no longer writes `.cursor` ([Cursor](../config/retired-keys.md#cursor)). A project whose `targets` still names it gets a `retired-target` warning on every sync
- `mars link --json` prints the sync's report with `target`, `local`, `settings_updated`, `synced` and `removed` added. There `synced` and `removed` count the linked target's files only, not the sync's totals (`mars sync --json`'s `removed` counts items); `targets[]` carries each target's own counts.
- Materialization depends on target, `agent_emission`, and optional `[settings.meridian.agent_copy]` (e.g. `.opencode` receives skills/MCP unless native agent emission or selective copy applies)

```bash
mars link .claude            # Copy managed content into .claude/
mars link .opencode          # Copy skills/MCP into .opencode
mars link agy                # Write an `agy` plugin into .agy and list it in .agents/plugins.json
mars link .claude --force    # Same as `mars sync --force`: adopts collisions in every target
mars link .agy --local       # Sync .agy on this machine only
```

---

## `mars unlink`

Stop managing a target and remove Mars-owned content from it.

```bash
mars unlink <target> [--local]
```

| Flag | Description |
|---|---|
| `--local` | Remove `<target>` from `mars.local.toml` only. A target `mars.toml` also names stays linked and keeps its files; one it does not name is removed as below |

**Behavior:**
- Removes `<target>` from `targets` in `mars.toml` and `mars.local.toml` (`--local`: `mars.local.toml` only)
- Runs the removal a sync runs for a target you took out of `targets` by hand: the next `mars sync` removes what Mars installed there the same way, so `unlink` is for removing it now and for a retired target the config still names
- Treats a target the lock still claims as managed, so an interrupted `link` or `unlink` can be finished
- Rejects `.mars`, the canonical store, which is not a target
- Removes only outputs and config entries owned by the target's lock records, which includes a hook's or MCP server's copy under `.mars/<surface>/<target>/` and its record on a target that takes none of it. For `.agy` that includes its entry in `.agents/plugins.json` and its keys in `.agy/plugin.json`; a file those entries leave empty is deleted when Mars's change emptied it, and so is `.agents/` when that leaves it empty
- Removes empty ancestor directories, including the target directory when it becomes empty; preserves handwritten or otherwise unowned content
- Removes an output only while it is what Mars wrote. One that differs from its recorded checksum (an edited skill, a file you added inside a skill directory, a tree with a dangling symlink) stays, loses its record, and is reported once per target (`output-kept`); `removed_outputs` counts what was deleted. An output that cannot be deleted (`output-remove-failed`) keeps its record and the rest go on; the next sync or `unlink` retries it
- Reports if the target was not managed (no-op)
- Works for a retired target (`.cursor`): it removes what the lock records, including the MCP servers and hook entries Mars added to `.cursor/mcp.json` and `.cursor/hooks.json` ([Cursor](../config/retired-keys.md#cursor))

```bash
mars unlink .agents          # Remove deprecated .agents target
mars unlink .claude          # Stop managing .claude
mars unlink .agy --local     # Stop syncing .agy on this machine; the project's own link stays
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

`mars models refresh` prints how many models it fetched and how many of them the project's
`catalog_providers` let through; `--json` carries both, as `fetched_models_count` (`null` when
it served stale data) and `models_count`.

`mars models refresh` fetches models.dev metadata synchronously into the per-user
catalog snapshot (`models-catalog-v2.json` in the Mars cache root, `MARS_CACHE_DIR`), shared by every
project on the machine. The snapshot holds every models.dev provider; a project's
`catalog_providers` filters what it reads, and its `models_cache_ttl_hours` decides when it asks for
a refresh. A machine with no snapshot is seeded, stale, from the project's old
`.mars/models-cache.json`, which is never written again. The catalog is indefinite last-known-good data:
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
`[settings.model_visibility]` in `mars.toml` or `mars.local.toml` is no longer read, like any
key Mars has no field for. A dependency package's consumer-only settings are ignored by
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

Each line is the rule's `match` kind and value, then its `when` text. `--json` returns `ref`, `ref_kind` (`agent` or `alias`), `agent_name`, `model_alias`, `model_name`, `found` and `prompting`. An unknown reference exits 1. See [`when`](../config/schema-v1.md#model-policies).

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
| `--root PATH` | The same directory, named as every other command names its root; `path` wins when both are given, so `meridian mars check <path>` (which appends `--root`) checks `<path>` |

Does not require a mars project (no `mars.toml` needed). It prints the findings in the package's files, one group per file in the order `mars.toml` first and then by path, each with its code and file and each group sorted errors first. What only the binary checks (the `requires-mars` floor, hooks, plugin output, model aliases, dependency resolution) prints after the groups, without a file. Errors exit 1; warnings and info do not. Info prints here and not in a default `mars sync`.

What its files say is the same function `@meridian-flow/mars-schema` exports as `check` (`mars_schema::check`). A level says what the author loses: an error, the file or package won't load or install; a warning, something written won't take effect; info, nothing written is lost.

| Code | Level | When |
|---|---|---|
| `package.requirement` | error | `requires-mars` or `requires-meridian` is not a version requirement (both are reported) |
| `definition.duplicate-name` | error | two Agents, or two skills, with one `name`; reported on each file |
| `skill-ref.missing` | error | an Agent names a skill no package of the resolved graph ships. A dependency's own Agent is `skill-ref.dependency` |
| `skill-ref.provider` | error | a ref `<skill>__<provider>` whose provider `[package] uses-providers` doesn't list |
| `skill.no-definition` | warning | `skills/<dir>/` holds files and no `SKILL.md`, so none of it is installed |
| `selection.duplicate-slug` | warning, info | two dependencies install one slug (warning); a root item and a dependency's (info: the root's is installed) |
| `field.restriction-lost`, `field.execution-ignored` | warning | a translated file lost a restriction, or an ignored `effort`, `mode` or `harness`. A sync reports it as a warning for a dependency's file too, once, with the sync that installs it |
| `agent.description-missing`, `agent.name-missing`, `definition.name-mismatch`, `frontmatter.repaired` | info | no `description` (Claude doesn't load such an Agent), no `name` (the file name is used), a `name` that differs from the file or folder, a value quoted so YAML could read it |

Every other finding is what `mars sync` applies: definitions against [schema v1](../config/schema-v1.md) (invalid values and malformed YAML are errors, a retired key is a `field.retired` warning, unknown keys are `info`, a package with no `schema` is an error), and a `skills` ref resolves through the package, its `[dependencies]` and their dependencies, fetched for the check. A dependency that can't be fetched is `[dependency-resolution]`, and one the check lacks after the fetch, or one that could not be read (`unreadable_dependencies` of the function's result), is `[dependency-incomplete]`; all are errors. While the function's `missing_dependencies` or `unreadable_dependencies` is not empty its result is not a verdict: no skill ref was judged. `check` ignores `.mars/provided.lock`, so the verdict is the same on every machine. A symlink is read as its target, as `mars sync` reads it.

`mars check` also reports what depends on a machine, a target or a Mars version, which the function does not:
- Package structure: convention folders named `agents/`, `skills/`, `hooks/`, or `bootstrap/` at non-hidden depth up to `MAX_DISCOVERY_WALK_DEPTH = 5`, plus package-root `SKILL.md` fallback; "no agents or skills found" is an error
- Configured plugin output: missing/skipped builds, stale inputs, a different package or Mars
  version, hand-edited or missing generated files, root files left holding an earlier build's
  output, and conflicts at generated package-root paths
- `[package].requires-mars` against the package's floor, which is the highest first-Mars among
  the keys it uses: `schema = 1` and `uses-providers` (0.20.0), `[plugin]` (0.19.0), `[hooks]` in a
  `hook.toml` (0.25.0), and a `deny` or `ask` in `[package.tools]` (0.30.0). A missing or lower
  requirement is an error that names the key and the line to write. See
  [mars.toml](../config/mars-toml.md#package-optional). This is about an older Mars, so `mars check` can fail a package `check` accepts
- Plugin compatibility: unknown `[plugin].harnesses` values
- Hooks (`hooks/<name>/hook.toml`) that use `[hooks]`: warns when an `if` sits on an event that is not a tool event (it never matches), and for each event, handler type or matcher a managed target cannot render. See [Declaring Hooks in a Package](../config/mcp-and-hooks.md#declaring-hooks-in-a-package)
- A model policy whose `alias` is not in the catalog, read from `mars.lock`
- The `source package validates` line, printed only when there are no errors

If a `mars.toml` is present in the target directory, also runs a **field-loss
preview** (`collect_prepared_source_lossiness_diagnostics`) against the configured managed
targets — reports what each native target cannot express from the source tree's
agents and skills.

`--json` prints `agents`, `skills`, `errors` and `warnings` (strings, with each warning-level finding of a file among them), and `diagnostics`: one `{ source_path, code, severity, path, message }` per finding in a file, info included, in the order above. A `skill-ref.dependency` entry also has `dependency`, with the Agent's output path as `source_path`. A finding of level info is in `diagnostics` only, except a `field.unknown` note, which is also in `warnings` as it was before `diagnostics` existed.

```bash
mars check                   # Check current directory
mars check ../my-agents      # Check another directory
```

---

## `mars version`

Version and release a package repository.

```bash
mars version <patch|minor|major|X.Y.Z> [--push] [--no-tag] [--force] [--no-plugins]
```

The command requires a clean Git worktree, runs the package publish checks, updates
`[package].version` and `CHANGELOG.md`, commits the release, and creates the annotated version
tag. `--push` pushes the current branch and tag after the local release succeeds.

`--no-tag` leaves tagging to CI: the command does everything else (checks, plugin rebuild,
commit, the refusal when the tag already exists) but creates no tag, and `--push` pushes only
the branch. `--json` then reports `"tag": null`; the text output prints `vX.Y.Z (not tagged)`.

When `[package].requires-mars` is missing or lower than the package's floor (what
[`mars check`](#mars-check) derives from the keys the package uses), `mars version` writes
`requires-mars = ">=<floor>"` in the same commit and says so; `--json` adds `requires_mars`. It
changes that value, or adds the line after `version`, and keeps every other byte of `mars.toml`.
It stops with the line to write when `[package]` is written as dotted keys. A `requires-mars` it
cannot parse is left for `mars check` to report.

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
- Each locked item of a managed target and of `.mars`: file exists on disk, no conflict markers, checksum computability
- Config-lock consistency: dependencies in config match lock entries
- Agent skill references: every declared skill dependency exists on disk
- Targets the sync does not manage (a retired one the config still names): their records and `.mars/<surface>/<target>/` copies are not checked
- Target health: each managed target has the expected files with correct content
- Target divergence: reports locally modified files (plain `mars sync` overwrites edited native agents and skills; edited hooks are kept until `mars sync --force`, which resets every diverged item in the project), missing files (`mars sync` restores them), items of the wrong kind (a file where a directory belongs, or the reverse) and items Mars could not read (fix the cause it names); see [troubleshooting.md](../dev/troubleshooting.md#target-divergence)

Exit code 0 = healthy, 2 = an error was found. A skill an Agent names that is not installed is the same warning (`missing-skill`) here as in `mars sync` and `mars validate`; it does not fail the check. With `--json`, `diagnostics` lists every finding with its `level`, `code` and `message`; `errors` and `warnings` keep listing the messages as before and are deprecated and stay for one minor release. See [troubleshooting.md](../dev/troubleshooting.md).

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

## `mars provide`

Install a host program's own skills, hooks and MCP servers.

```bash
mars provide <provider>@<version>=<directory> [--root <dir>] [--json]
```

A host ships its content with its binary and hands Mars the directory holding it
(Meridian runs this on every launch). Mars copies `<directory>/skills/*` into
`.mars/skills/` and records the names in `.mars/provided.lock`, which no other
command writes. It also validates `<directory>/hooks/*` and `<directory>/mcp/*` and
stages them at `.mars/provided/<provider>/`. It never touches `mars.toml`, `mars.lock`
or a managed target: the next `mars sync` installs provided skills, hooks and MCP servers
into the linked targets whose harness Meridian does not launch (`.agy` today) and into no
other. A launched harness reads `.mars/skills/` itself. `--root <dir>` must hold
its own `mars.toml`; a directory without one is refused rather than walked up
from, so a host always fills the root it named.

| Field | Meaning |
|---|---|
| `<provider>` | The host's own namespace, separate from dependency keys. `[a-z0-9][a-z0-9-]*` |
| `<version>` | Opaque, non-empty; recorded for display and change detection (host versions need not be semver) |
| `<directory>` | Must exist. Holds any of `skills/`, `hooks/` and `mcp/`. A skill is a directory named `<skill>__<provider>` whose `SKILL.md` has the same `name`; a hook is `hooks/<name>/hook.toml` with `visibility = "exported"`; an MCP server is `mcp/<name>/mcp.toml` |

Entries starting with `_` or `.` are ignored, as are top-level files, so
a Python package's `__init__.py`, `__pycache__/` and `AGENTS.md` are not entries.
A directory with none of the three provides nothing: it removes every skill the provider
recorded and its staged hooks and MCP servers, and the next sync removes what they installed.
That is how a host retracts; there is no `--remove`.

A skill is compared by each file's bytes and executable bit, since an installed copy
is `0o644` plus the source's execute bits. Every installed file gets that mode, so
adding or removing a file's execute bit replaces the installed skill on the next run, with or without a version bump.

`--json` prints `status`, `provider`, `version`, `skills` (the recorded names
after the run), `installed`, `replaced`, `adopted`, `removed`, `shadowed`,
`collisions` and `warnings`. `status` is one of:

| Status | Meaning |
|---|---|
| `up-to-date` | A re-plan would write nothing: `provided.lock` and the installed trees already match the source |
| `applied` | Skills were installed, replaced, adopted or removed |
| `busy` | Another Mars process holds `.mars/sync.lock`; nothing was written |
| `sync-pending` | An interrupted sync owns canonical destinations; run `mars repair` (or `mars sync`) to recover, then provide again |

All four exit 0. Validation failures exit 2 before anything is written:
`provided-name-invalid` (a name or `SKILL.md` `name` that is not `<skill>__<provider>`),
`provided-skill-invalid` (a missing directory or `SKILL.md`, or one the schema rejects),
`provided-hook-invalid` (a `hook.toml` the reader rejects, including one without
`visibility = "exported"`) and `provided-mcp-invalid`.

Mars only deletes names its own record lists. A directory it did not create is
adopted when its bytes match the source and reported as `collisions` otherwise; a
provided skill whose destination a package owns is reported as `shadowed` and left
to the package. Both are reported on every run while they last, from the fast
path and the locked one alike, not only on the run that first saw them. Plain
text prints source diagnostics (schema warnings) only for `applied`; `--json`
carries them in `warnings` on every status.

Mars's records of what sync installed from a host are machine-local too, in
`.mars/machine.lock` (the schema of `mars.lock`), never in `mars.lock`. A provided hook, MCP
server or skill whose name a package or `.mars-src/` also provides is skipped with a
`provided-shadowed` warning, and the package's is installed. A provided hook in a hook
schema this Mars retired keeps what the last sync installed and warns; it never fails
`sync`, `upgrade` or `remove`, and `--frozen` ignores provided content.

`.mars/provided.lock` is machine-local (`version = 1`, one `[providers.<name>]`
table per provider with `version`, `digest` and `skills`). `digest` is `pending`
while an apply is in flight; the next run replans from the files on disk. The
file is written by another build, so a recorded name must be a single path
component ending in `__<that provider>`; anything else makes the file corrupt
(one warning, an empty record) rather than letting a record point a removal at
another provider's directory.

`mars sync` reads it and never writes it. It counts a recorded name as available for refs from a
package that declares the provider in `uses-providers` (see
[mars.toml](../config/mars-toml.md#provided-skills-and-uses-providers)) only while
`.mars/skills/<name>` is a directory, and treats a recorded destination as Mars-owned, so a
package item or `.mars-src/` item that wants the same destination replaces it instead of
colliding. A corrupt or newer `provided.lock` reads as "nothing provided" with one
`provided-lock-unusable` warning; a missing one is silent. `mars doctor` warns, rather than
errors, on a missing `<skill>__<provider>` ref, since it has no manifest to read declarations from
and does not read the record: it scans the canonical store. See
[canonical ownership](../internals/lock-file.md#canonical-ownership) for the rule the two records
share.

---

## `mars cache`

Manage the global source cache.

```bash
mars cache <info|clean>
```

### `mars cache info`

Show cache location and disk usage (total, archives, git clone cache; `archives` holds one directory per fetched commit).

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
mars build plugin [PATH] [--out <dir>] [--verbose]
```

`PATH` defaults to the detected project root. A relative `PATH` resolves against the working directory when it exists there; otherwise, with an explicit `--root`, against that root. `--verbose` lists every field a tree's agents do
not carry; by default only what you wrote and can fix is reported (see
[lossiness](../config/agent-compilation.md#lossiness-model)). With an explicit path the command can run outside
a Mars project. The build uses only `mars.toml` publish state: it does not read `mars.local.toml`,
the user `config.toml`, `mars.lock`, `.mars/`, project target configuration, or installed harness
model catalogs.

Without `--out`, `[plugin]` is required and the command writes `[plugin].dir` (default `plugin`).
The configured build also writes the Claude marketplace catalog plus the root OpenCode
`package.json`. With `--out`, Mars writes only the output directory and accepts a package without
`[plugin]`; an explicit destination cannot resolve to the configured directory.

The output is an owned nested artifact:

```text
<dir>/
  claude/    # Claude, Codex, and Grok
  opencode/  # OpenCode
  .mars-plugin.json
```

Only trees selected by `[plugin].harnesses` are written. The shared Claude tree contains native
agents, skills, exported hooks, exported MCP servers, and `.claude-plugin/plugin.json`; Codex
loads its skills, hooks, and MCP servers but not its agents. OpenCode receives an npm-shaped package that registers skills, agents and MCP servers and
composes default-export native hooks. `[hooks]` reaches only the shared Claude tree; the OpenCode tree drops it with `plugin-native-hook-dropped` unless the hook has a native hook for that tree (see [Plugin builds](../config/mcp-and-hooks.md#plugin-builds)).

Mars records generated file hashes, input and dependency identities, version, and root-file
ownership in the sentinel. It atomically replaces owned output, recovers interrupted staging, and
refuses a non-empty directory or root file it does not own. See
[Authoring harness plugins](../plugin-authoring.md).

### `mars build launch-bundle`

Build a harness-targeted launch bundle — a JSON artifact containing routing, execution policy, prompt surface, and tooling metadata for spawning an agent.

```bash
mars build launch-bundle [--agent NAME] [--model TOKEN] [flags]
```

A project whose `min_mars_version` the running Mars does not meet fails with `[compat-version]` and exit 2, and prints no bundle. A `min-mars-version-low` warning (a file that uses a newer key without a `min_mars_version` that says so) belongs to the author: `mars sync` prints it and the bundle's `warnings[]` does not.

**Two modes:**

**Ad-hoc mode** (no `--agent`):
- Requires `--model <token>`
- Does **not** require a `mars.toml` project — runs from any plain directory
- Uses no agent profile; routing is driven by `--model` and the installed harness environment

**Agent/profile mode** (`--agent <name>`):
- Reads `.mars/agents/<name>.md` from the nearest `mars.toml` project
- Requires a project with `mars sync` already run
- Inherits the profile's fields (model, harness, effort, approval, sandbox, network, skills, tools, model-policies)
- Model precedence is `--model` > the `[agents.<name>]` overlay > profile model > `settings.default_model`.
- Effort, approval, sandbox and network follow the [precedence chain](../config/schema-v1.md#model-policies): CLI flag, overlay field, the first matching `model-policies` rule, the profile's own value, then (`network` only) the `[settings]` and user-config `default-network`, or (`effort` only) the alias's `default_effort`.

| Flag | Description |
|---|---|
| `--agent <name>` | Agent name from `.mars/agents/<name>.md`. Omit for ad-hoc mode. |
| `--model <token>` | Model token or canonical model ID. Required in ad-hoc mode; overrides profile in agent mode. |
| `--harness <name>` | Override harness target (`claude`, `codex`, `opencode`, `pi`; `agy` is refused: Meridian does not launch it). |
| `--effort <level>` | Override effort: `auto`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. `auto` sends no effort; `none` asks for reasoning off. The bundle carries the level the model and harness take, with a warning when it moved; see [Effort](../config/schema-v1.md#effort). |
| `--config-effort <level>` | The caller's own configured effort (Meridian's `primary.effort`), same values as `--effort`. Below the Agent's and the model policies' effort, above the alias's `default_effort`; its source is `config`. |
| `--approval <mode>` | Override approval: `default`, `auto`, `confirm`, `never`. |
| `--sandbox <mode>` | Override sandbox: `default`, `read-only`, `workspace-write`, `danger-full-access`. |
| `--network <mode>` | Override shell network mode: `allow`, `deny`, `allowlist`. Built-in web tools remain controlled by `tools: web`. |
| `--network-allow <host>` | Add an allowed host. Repeatable; accepts exact hosts, IPs, `*.x`, and `**.x`. |
| `--network-deny <host>` | Add a denied host. Repeatable; uses the same host grammar. |
| `--skill <name>` | Add extra skills by name. Repeatable; also accepts `--skill a,b`. |
| `--agent-tool-rules <path>` | JSON file (`-` reads stdin) of tool rules that replace the agent's own profile and overlay rules; see below. |

**Supplying the agent's tool rules (`--agent-tool-rules`):** a caller that stored an agent's rules (a session that continues) passes them back instead of combining them with `[settings.tools]` itself. The file is a JSON array; each entry has `capability`, `pattern` (string or `null`) and `action` (`allow`, `deny`, `ask`), and `origin` (`profile`, `overlay` or `package`), the fields of a bundle's `tools.rules` entry, so rules a bundle emitted can be passed back. Other fields are ignored; an entry of origin `project` is skipped, and any other `origin` fails the command like any bad entry. A `*` rule is `capability: "*"`; an MCP rule is `capability: "mcp"` with `pattern: "server/tool"`. The rules replace the agent's profile and overlay tool rules, order and origin kept, with or without `--agent`; the current `[settings.tools]` is then applied and projected as usual, so `rules`, `covered_by`, `native`, `allowed` and `disallowed` come out as for a profile that had written those rules. The supplied rules also stand in for the `[agents.<name>.tools]` overlay's rules, but with `--agent` an overlay rule entry replaces the same key in `[settings.tools]` only when the supplied rules also hold an `overlay` rule with that key and action (`allowed` and `disallowed` never did). An exemption added or changed after the rules were stored therefore applies from the next new session; until then the project rule joins and the strictest decides. Without `--agent` no exemption applies. An entry that does not parse fails the command with its index and the reason, exit 2, and prints nothing.

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
    "catalog_row": { "provider_key": "anthropic", "id": "claude-opus-4-8", "cost_input": 5.0, "cost_output": 25.0, "cost_cache_read": 0.5, "cost_cache_write": 6.25 },
    "route_trace": { "version": 3, "model_attempts": [{ "selected_by_preference": false }], "..." }
  },
  "execution_policy": {
    "effort": "high",
    "approval": "auto",
    "sandbox": "workspace-write",
    "network": { "mode": "allow", "allow": [], "deny": ["blocked.example"] },
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
      { "capability": "edit", "pattern": null, "action": "deny", "origin": "profile",
        "native": ["Edit", "Write", "NotebookEdit"],
        "native_tools": ["Edit", "Write", "NotebookEdit"], "native_status": "known" },
      { "capability": "bash", "pattern": "rg *", "action": "allow", "origin": "profile",
        "native": ["Bash(rg *)"], "native_tools": ["Bash"], "native_status": "known" }
    ],
    "allowed": ["Bash(rg *)"],
    "disallowed": ["Edit", "Write", "NotebookEdit"],
    "mcp": [],
    "opencode_permission": [
      { "permission": "bash", "pattern": "rg *", "action": "allow" }
    ]
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
    "network_source": "cli|overlay|overlay-model-policy|profile|profile-model-policy|settings-model-policy|config|unset",
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
- `execution_policy` holds `effort`, `approval`, `sandbox`, `network`, `autocompact`, `autocompact_pct` and `timeout`. Compaction comes only from the model alias; an Agent has no compaction setting. `effort` is the resolved level, or `null` when none is sent (`auto`, a model that does not reason, or Antigravity, which folds the level into `harness_model`); a request the model or harness does not take moves to the nearest level they share, with a `launch.warning`. `routing.catalog_row` is the models.dev row of the model the harness runs, `{provider_key, id, cost_input, cost_output, cost_cache_read, cost_cache_write}`, or `null` when there is none; read prices from it instead of joining the catalog yourself.
- `execution_policy.network` is one resolved `{mode, allow, deny}` object. `mode` is
  `"allow"`, `"deny"`, `"allowlist"`, or `null`; Meridian resolves a remaining unset mode
  from the sandbox. Mars removes inert sets (`deny` removes both; `allow` removes `allow`) and
  turns an empty allowlist into `deny` with a warning. The field governs shell connections
  only; `tools: web` separately governs built-in web tools. See [Network](../config/schema-v1.md#network).
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
| `origin` | Where the rule came from: `profile` (the agent's own), `overlay` (a rule entry in `[agents.<name>.tools]`), `package` (the package's `[package.tools]`, added to an Agent that does not name the key) or `project` (`[settings.tools]`; also an `ask` cut down to an agent's own allow, when the agent closes `*`). Not part of a rule's identity: a profile rule and a project rule with the same key and action are one rule, and it keeps the profile's origin |
| `native` | The harness's permission names for the rule, after scoped-name and conflict resolution. Empty unless `native_status` is `known`; a known rule is also empty when a stricter rule covers it or owns the shared permission. Already resolved: `edit: deny` + `write: allow` on Claude gives the `edit` deny `Edit`, `Write`, `NotebookEdit` and the `write` allow none, so emit `native` as given |
| `native_tools` | The unscoped harness tool names concerned by the surviving `native` permissions. Never strip patterns or otherwise derive this field from `native`. For a `*: deny` allowlist, the availability set (such as Claude `--tools`) is the union from known `allow` and `ask` rules; a `deny` rule never grants availability. Empty whenever `native` is empty |
| `native_status` | `known`: Mars knows the harness permission, whether it survives conflict resolution or is shadowed. `absent`: the harness has no such tool, nothing to enforce. `unknown`: Mars cannot name it for this harness, a gap to report. Omitted for `*` |
| `covered_by` | Present only when a stricter rule wins every call this rule would match, or every name it has on this harness: `{"action": "deny", "key": "bash(uname *)"}`, the covering rule as its author wrote it. The rule never applies, and `native` and `native_tools` are empty. Test for it with this field, not with an empty `native`. A reader that ignores it stays safe: a covered rule carries no native name to emit. Omitted when the rule is not covered |
| `note` | A harness-specific consequence the consumer must surface. On an `unknown` rule, why Mars could not name it (`"codex loads skills by reading files; no switch"`); on a `known` rule, why the valid native permission is broader, or why a stricter rule left it no native name. Permission gaps never add an entry to `warnings[]`: `note` is the only channel |

**`tools.opencode_permission`** is present only when the resolved harness is OpenCode: the
`permission` map Mars writes to the agent's native file, as a list in the order OpenCode's
last-match rule reads it. Each entry is `{permission, pattern, action}`; a map key with a plain
value has pattern `*`, and a `*` entry comes first. It comes from the same function as the
file, over the same effective `rules` (the profile, the overlay, `[settings.tools]`, or
`--agent-tool-rules`) and the author's `harness-overrides.opencode.permission` in written
order, and it is never looser than either. An agent with neither gets `[]`. A launcher appends
what only it knows after these entries (a refusal in place of `ask` when headless, its own
grants and denies). `execution_policy.native_config.permission` still carries the author's
override with its keys sorted, as before; read the list, not that.

**`tools.set_aside`** is always `[]`. It listed the rules a project-wide deny covers; those now
stay in `rules` with empty `native` and `native_tools`, a `covered_by` and a `note` naming the stricter rule
(see [Project-wide tool rules](../config/mars-toml.md#project-wide-tool-rules)). It stays for one
release so a reader of the field keeps working.

`tools.allowed`, `tools.disallowed` and `tools.mcp` are flat native-name lists derived from
`rules` for consumers that predate it. They cannot express `ask`, `*`, or the capability behind
a name; `allowed` never reflects an allowlist, and `mcp` never contains server definitions or
commands. Prefer `rules`. A native name that rules of different actions share goes to the
strictest, so no name is in both `allowed` and `disallowed`.

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
