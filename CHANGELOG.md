# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added
- `harness-overrides.meridian` is a reserved consumer namespace, as `harness-overrides.flow` is for
  Flow. Mars keeps the block in `.mars/agents/` and never writes it to a native file, a plugin tree
  or a launch bundle; its keys belong to the Meridian CLI: `resident-rearm-budget` (an integer) and
  `capabilities` (a map).

### Changed
- Agent `resident-rearm-budget` and `meridian-capabilities` are removed top-level keys: a schema 1
  file that carries one is a `field.removed` error naming
  `harness-overrides.meridian.resident-rearm-budget` and `harness-overrides.meridian.capabilities`.
  `mars migrate` moves both with their values, so a pre-v1 package and a schema 1 file that still
  carries them migrate without a hand edit.
- `mars migrate` rewrites a package that already declares `schema = 1` when it still carries a key
  that moved under a `harness-overrides` namespace (`permission`, `resident-rearm-budget`,
  `meridian-capabilities`), instead of failing the run on it. Everything else holds: values and the
  file's key order are kept, `harness-overrides` keeps the place the author gave it, a block that
  already sets the destination key wins and the dropped top-level key is named, and any other
  removed key still fails and names the file and the key.

## [0.20.6] - 2026-10-27

### Fixed
- `mars build plugin` clears the sentinel's `root_files_previous` recovery record once the root
  files are written, so a package whose root `package.json` changes (every version bump) is
  byte-identical to an immediate rebuild instead of needing a second build. A publication killed
  between the tree swap and the root-file rewrite is still resumed by the next build.

## [0.20.5] - 2026-10-27

## [0.20.4] - 2026-10-27

### Added
- `${MARS_MCP_DIR}` in an MCP server's `command` or `args` names its own `mcp/<name>/` files.
  `mars sync` installs that directory beside each target that writes MCP and substitutes the
  installed absolute path; `mars build plugin` copies it into each tree and maps the token to
  the harness plugin root (or resolves it at runtime in the OpenCode tree). A server that never
  names the token registers and copies exactly as before, and a target that writes no MCP
  config receives no directory. Executable bits are preserved and a removed server prunes its
  installed directories. A token-using server's name must be a single path component; other
  names skip that one server with a warning.

### Changed
- Anything unmanaged at `<target>/hooks/<name>/` no longer fails `mars sync`, whether it is a
  directory or a file where that directory belongs. It warns, survives untouched, and withholds
  that target's hook entry, as an MCP server's unmanaged `mcp/<name>/` already did;
  `mars sync --force` adopts and overwrites it.

### Fixed
- A dependency narrowed by any include filter (`agents`, `skills`, `only_skills`, `only_agents`)
  now still contributes its exported MCP servers, instead of silently dropping every server it
  declares. Filters select agents, skills and hooks; they never named servers. Upgrading: a
  newly visible server whose name matches an entry you wrote by hand in that target's MCP config
  replaces that entry.
- `mars build plugin` keeps the building package's own agent and skill names when a dependency
  defines the same name: the root ships `rev.md` and the dependency `rev__adep.md`, instead of
  both being renamed to `rev__pkg.md` and `rev__adep.md`.
- A FIFO, socket or device inside a skill, hook or MCP server directory is rejected with an error
  naming it, instead of hanging `mars sync` while hashing the tree. File errors from that hash walk
  now name the path instead of `<unknown>`.
- Editing an installed hook or MCP server directory under `.claude`, `.codex`, `.opencode`, `.pi`
  or `.cursor` now reports `target-divergent` with the `mars sync --force` / `mars repair` hint, as
  skills already did on targets without skill variants; the edited copy is kept and its config
  entry is withheld.

## [0.20.3] - 2026-10-27

### Fixed
- A generated-plugin sentinel with a Windows-absolute path entry (`C:\x`, `\\server\x`, `/x`) is rejected on every host, and `root_files_previous` entries are validated like `root_files`. `mars-schema` now owns the one generated root-file table and sentinel validators that the binary and `mars check` use, so a new plugin tree cannot be accepted by one copy and refused by the other. The binary reports the reason `mars-schema` gives, which names the offending entry, instead of testing the rules a second time.

## [0.20.3-rc.1] - 2026-10-27

### Fixed
- `@meridian-flow/mars-schema` publishes to npm again: its `package.json` names the source repository, which npm needs to verify provenance.

## [0.20.2] - 2026-10-27

### Fixed
- `mars sync` no longer fails with `Not a directory` on a dependency read without a `mars.toml` (a foreign skill tree, a `subpath` into another repo) whose `skills/` holds a file such as `__init__.py` or `README.md`; the file is ignored, as in 0.18.
- `mars check PATH --root DIR` checks `PATH` instead of failing, so `meridian mars check <package>` works again.

### Changed
- README plugin support table: cells are generated from what each plugin tree actually does. Grok
  agents now list the Claude agent fields that are dropped; Codex and Grok hooks state the `claude`
  fragment rule. Generated plugin output is unchanged.

## [0.20.1] - 2026-10-26

### Fixed
- An installed `.mars` tree an older Mars wrote reads again without `mars sync` first: when an installed Agent or skill reports a pre-v1 spelling, it is upgraded in memory exactly as a pre-v1 package is and read again, so a skill's `type` or an Agent's `model_invocable` no longer drops it from the launch bundle, `mars skills`, `mars agents` or `mars models prompting`. Schema v1 text and files that are invalid rather than pre-v1 read as before; authored packages that declare `schema = 1` stay strict.
- Codex gets `effort: max` as `max`: its models accept the level, so the native file and the launch bundle no longer write `xhigh` and warn that they approximated.
- `mars migrate` adds `[package] requires-mars = ">=0.20.0"` to a package that has no requirement, so a migrated schema v1 package needs no hand edit. It never rewrites an existing one: a requirement that allows a Mars older than 0.20.0 is reported with `mars check`'s warning and left as authored, and so is a manifest it cannot edit minimally (an inline `[package]` table).

## [0.20.0] - 2026-10-26

Schema v1: one versioned format for agent definitions, skills and the `[package]` table, read by `mars-schema` and shared with the npm package. See [Schema v1](docs/config/schema-v1.md) and [Migrating a package](docs/config/migrating-to-schema-v1.md).

### Breaking
- Sync reads every package as schema v1 before writing any installed content. An error (removed key, invalid value, `tools` list form, unclosed or non-mapping frontmatter, duplicate key, YAML anchor, `yes`, `no`, `on` or `off` in a boolean field) now fails `mars sync`, `mars add` and `mars upgrade` with exit 2 and changes no installed content, target file or `mars.lock` (a fresh project still gains `.mars/sync.lock`); `mars check` exits 1. Before, sync installed such files with a warning. Unknown keys still warn and are kept. A package with no `schema` in `[package]` is upgraded in memory with one warning that names it; a tree with no `mars.toml` is translated by its dialect's rules, and a key or value translation cannot make schema v1 moves under `harness-overrides.<dialect>` (or is dropped when no harness reads it) with a warning that names it, so only an authored schema v1 file is refused for its fields.
- Agent keys removed: `disallowed-tools` (write `tools: {name: deny}`), `fanout` (use `[settings.meridian.fanout]`), `models`, `mcp-tools`, `autocompact`, `autocompact_pct`, `model_invocable`, `user_invocable`, `network_allow` and `network_deny` (write `network-allow`, `network-deny`), a `tools` list, `permission` (now `harness-overrides.flow.permission`) and `approval: yolo` (now `never`).
- Skill keys removed: `type` (now `metadata.type`; the launch bundle still reports `skill_type`), `when_to_use`, `tools`, `disallowed-tools`, `allowed-tools`, `model_invocable`, `user_invocable`, `invocation`, `disable-model-invocation`, `allow_implicit_invocation`. Skills carry no tool policy; `[skills.<name>.tools]` overlays are deleted.
- `package.visibility` removed from the manifest.
- Compaction comes only from `[models.<alias>]`. Agent and overlay `autocompact`/`autocompact_pct`, and `autocompact`, `autocompact_pct` and `harness` in `model-policies` overrides, are gone. A `model-policies` override sets only `effort`, `sandbox`, `approval` and `network`.
- Precedence for effort, sandbox, approval and network: CLI, `[agents.<name>]` overlay, matching `model-policies` rule (overlay, then profile), the profile's own value, `[[settings.model-policies]]`, `[settings]` `default-network` (network only), alias `default_effort` (effort only). A profile rule's override now beats the profile's top-level value, which was the reverse. Example: `meridian-dev-workflow`'s `coder` has `effort: max` and a `deepseek` rule with `override: {effort: high}`; with `deepseek` its bundle's effort was `xhigh` and is now `high`, and `explorer` goes from `medium` to `low` the same way. To keep the top-level value, delete the rule's `effort` override.
- An `[agents.<name>]` overlay with `approval = "yolo"` fails `mars.toml` loading. v0.19.0 passed `yolo` into the bundle. Write `never`.
- A marketplace dependency with several plugins and no `plugin` is an error that lists them. Add `plugin = "<name>"` to the dependency entry (or run `mars add <marketplace> --plugin <name>`). A marketplace with one plugin keeps working without it.
- After upgrading Mars, `mars sync --frozen` fails once with `lock file would change`: schema v1 changes the installed bytes of pre-v1 packages and so their checksums. Run `mars sync` once and commit `mars.lock`.
- The launch bundle's `execution_policy.effort` can be `none`; the bundle stays version 4, so a consumer that maps efforts must handle it.

### Added
- `mars provide <provider>@<version>=<directory>` installs a host program's own skills into
  `.mars/skills/` and records the names in `.mars/provided.lock`, a machine-local file no other
  command writes. Nothing committed changes: `mars.toml`, `mars.lock` and every managed target stay
  byte-identical, and provided skills are never projected. A fast path answers `up-to-date` without
  the sync lock only when a re-plan would write nothing — the record matches the source and every
  source skill is either claimed by a package or installed with its source bytes and executable
  bit (installs are `0o644` plus the source's execute bits, so flipping a script's execute bit replaces the skill) — so a skill left
  out earlier (shadowed, collided) installs once the blocker is gone, and a run whose plan changes
  nothing writes nothing. A held lock answers `busy`, an interrupted sync answers `sync-pending` until `mars repair` or `mars sync` recovers it,
  and a crash mid-apply converges on the next run, which reclaims a skill directory the killed run
  left staged under `.mars/skills/`. `--json` reports the status, the recorded names
  and per-name `installed`/`replaced`/`adopted`/`removed`/`shadowed`/`collisions`; shadowed names
  and collisions are reported on every run while they last. `FileLock::try_acquire` is the one
  non-blocking lock helper now shared with the model probing caches, which log a lock error and
  skip the cache instead of reading it as contention.
- `[package] uses-providers = ["meridian"]` names hosts that supply skills at sync time; each entry
  must match `[a-z0-9][a-z0-9-]*`. A dependency's agent may list `<skill>__<provider>` for a
  declared provider: the ref resolves, nothing is pulled, and sync continues with
  `provided-skill-unavailable` where the host has not run `mars provide` (silent where it has).
  A ref to an undeclared provider stays `skill not found` on every machine, with a conditional
  `uses-providers` hint, as does a typo. The resolver never reads `.mars/provided.lock`, and
  `mars.lock` is byte-identical with or without a host. See
  [mars.toml](docs/config/mars-toml.md#provided-skills-and-uses-providers).
- A package item or `.mars-src/` item that wants a destination a provider installed replaces it,
  instead of `unmanaged-collision` or "blocked by unmanaged destination"; the next `mars provide`
  reports it `shadowed` and drops its record. Sync treats a recorded destination as Mars-owned
  through one ownership rule (`mars.lock` claim or provided).
- In a package repository, sync warns `provided-skill-undeclared` for a local agent ref to a
  provided skill whose provider is not in `uses-providers`. `mars check` (the publish gate, which
  ignores `.mars/provided.lock`) passes a declared `<skill>__<provider>` ref and fails an undeclared
  one naming `uses-providers`, with or without `[dependencies]`. `mars doctor` scans the canonical
  store and warns, instead of erroring, on a missing `<skill>__<provider>` ref; it does not read the
  record. Sync reads a corrupt or newer `provided.lock` as nothing provided, with one
  `provided-lock-unusable` warning; a missing one is silent. A recorded
  name counts as provided only while `.mars/skills/<name>` is a directory, so sync,
  `mars validate --strict` and `mars doctor` agree when a package replaced a provided directory and
  was later removed; ownership still follows the record, which covers directories a crash has not
  left behind.
- `[package] schema = 1`, the schema v1 format. It can still change in a 0.x minor release, listed in the CHANGELOG; from Mars 1.0 a change in meaning needs schema 2.
- `mars migrate [--check]` upgrades a package's authored definitions to schema v1 in place. It rewrites only the top-level keys that change and keeps every other byte, including a body whose line endings differ from the frontmatter's; a rare full rewrite re-renders the frontmatter only and copies the body as authored. It never touches dependencies, `.mars/` or `mars.lock`, and in a consumer project migrates only `.mars-src/`.
- `mars-schema` crate and `@meridian-flow/mars-schema` npm package (WebAssembly) with one reader for agents, skills and `mars.toml`: `read` returns a plain-object reading (file spellings, `null` for absent, `Uint8Array` bytes, mapping keys in file order) with stable diagnostics keyed by structured path; `emitValue` and `rewriteBlock` are a quote-safe YAML emitter for lossless editors; `vocabulary` publishes the tool capabilities, their aliases and each host's native tools, with a typed Flow table; `prepare`, `inspect` and `normalize` read a whole repository as packages. Fixture suites pin every behaviour and run in Rust and Node.
- `inspect` and `normalize` read Claude plugins, marketplaces (`strict` and `source` rules) and bare skill trees as schema v1 packages with relocated `skills/<slug>/` and `agents/<slug>.md`, a generated `mars.toml` and provenance; commands, hooks and MCP definitions are reported as not imported. Skill tool keys are dropped with a warning, `type` and `version` move to `metadata`, an unquoted `description:` colon is repaired, and Agent `disallowedTools` merge into `tools`. Dependencies without `mars.toml` go through `normalize`; `agent-skills` is a dialect.
- `plugin = "<name>"` on a dependency installs one plugin of the Claude marketplace at its `url` or `path`; each plugin is its own dependency, package and lock entry. A marketplace with exactly one plugin needs no `plugin`; with several, a dependency without one is an error that lists them. A plugin in another repository (`github`, `url` and `git-subdir` sources, at the entry's `sha`, else `ref`, else default branch) is fetched like any git source; `npm`, `archive` and `command` sources are refused naming the shape. `mars.lock` records the plugin and its content commit beside the marketplace's: `mars sync` replays both, `mars upgrade` re-reads the marketplace and resolves the content anew and reports a plugin whose source moved, and `--frozen` requires the locked content commit. Only a plugin dependency's lock entry gains `plugin` and `content`. `mars add <marketplace> --plugin <name>` writes the entry.
- `effort: none` (and `--effort none`) asks for no reasoning effort and stays distinct from absent end to end. Codex writes `model_reasoning_effort = "none"`; Claude has no off value, so its agent file omits `effort` and reports the loss; OpenCode, Pi and Cursor write no effort field.
- `when` on a `model-policies` rule (one non-empty line) describes when the rule's model fits. `mars models prompting @agent` prints the Agent's guide ahead of the alias's text, and the Agent inventory marks Agents that have one. `mars check` warns on a policy alias it cannot resolve, naming the agent and file.
- `ls` and `work` are tool capabilities. `harness-overrides.flow` is a reserved namespace for Flow that Mars passes through and never interprets.
- Reference pages for [schema v1](docs/config/schema-v1.md) and [migration](docs/config/migrating-to-schema-v1.md), and READMEs for the crate and the npm package.
- `mars check` warns when committed plugin output was built by another Mars version than the running one (the sentinel's `mars_version`): a Mars upgrade can write different files for the same inputs, which the input fingerprint cannot see. Rebuild with `mars build plugin`.
- `mars check` warns when a package that declares `schema = 1` has a `requires-mars` that allows a Mars older than `0.20.0`, the first release that reads schema 1. A plugin package with no `schema` keeps the `0.19.0` floor; the warning names one floor, the higher. v0.19.0 built `schema = 1` packages without complaint, and wrongly.

### Changed
- Installed files under `.mars/` get mode `0o644` plus the source's execute bits, the rule target projection already used; before, a skill, hook or bootstrap doc copied the source's full mode, so a `0600` file stayed unreadable to the group.
- Mars prepares every package (dependencies, `.mars-src`, the local package, `mars check`, `validate`, `migrate`, publish inputs) through `mars_schema::prepare` before it mutates anything. The authored tree stays untouched; sync projects prepared bytes into the recoverable `.mars/.staging.pending` generation after the diagnostic gate and publishes it by rename. Relative path dependencies resolve from the authored origin. Pre-v1 `.mars-src` content is upgraded in memory with a warning that names `mars migrate`.
- A package declaring `schema = 2` or higher is one `schema.newer` error ("package declares schema N; this Mars reads schema 1 — upgrade Mars") and nothing else.
- Frontmatter is bounded: a block over 64 KiB (`frontmatter.too-large`) or with more than 64 open brackets (`frontmatter.too-deep`) is not parsed. Nested brackets stalled the YAML scanner for seconds at 64 KiB. `read`, `prepare` and the npm package apply the same limits.
- Sync targets, lossiness preview, plugin builds and `doctor` take Agents and skills from the prepared package, not a second walk of the materialized tree; the binary's walk finds only hooks and bootstrap documents.
- Native agent files (`.claude`, `.codex`, Cursor, plugin trees) resolve effort, sandbox, approval and network through the launch bundle's chain, minus the CLI layer, so `[agents.<name>]` scalars, the matching `model-policies` rule and `[[settings.model-policies]]` apply; an artifact that names no model gets only the overlay and top-level values.
- A project's `[settings]` network (`default-network`, `network-allow`, `network-deny`) is reported once per Codex target (`project default-network is not projected to .codex`), not as a dropped field on every Agent; an Agent's own `network` keeps its per-Agent warning. Plugin builds ignore the package's own `[settings]` network.
- `[agents.<name>]` and `[skills.<name>]` overlays are consumer configuration outside the schema; a dependency's own overlay tables are inert. Keys they no longer read are ignored with a warning that names them.

- An unknown key in an Agent or skill reaches no native file (skills used to pass it to every harness); the `field.unknown` warning says to use `harness-overrides.<harness>`. Skills read `harness-overrides` like Agents.
- `harness-overrides.<harness>` is written into that harness's native Agent and skill files and plugin trees (Codex: extra TOML keys), never another harness's. A key Mars lowers itself keeps Mars's value and the override is dropped with a warning.
- Importing an OpenCode agent maps `tools: {write: false}` to `allow`/`deny` and keeps `permission`, `temperature` and the other OpenCode-only keys under `harness-overrides.opencode`; it failed `mars sync` before and v0.19.0 installed it.
- Importing a Claude plugin keeps its Claude-only keys instead of deleting them: `color`, `maxTurns`, `permissionMode`, `memory`, `background`, `isolation`, `initialPrompt` and `hooks` on Agents, and `argument-hint`, `arguments`, `model`, `effort`, `context`, `agent`, `hooks`, `paths` and `shell` on skills move unchanged to `harness-overrides.claude`, with an info message (`agent.claude-keys-moved`, `skill.claude-keys-moved`) in place of the old `*.claude-keys-dropped` warning; `mars sync` prints one line per package with the count of files translated.
- `mars migrate` prints each key it removed, by code and file (and as `dropped` in `--json`), and maps `adaptive`/`disabled` effort in `model-policies` overrides as it does in `effort:`.
- `[agents.<name>]` `effort`, `approval`, `sandbox` and `network` are checked when `mars.toml` loads; stale overlay keys (`autocompact*`, `tools.mcp`, `[skills.x.tools]`) warn by name, in `mars.toml` and `mars.local.toml` alike. The error lists `max`.
- The pre-v1 upgrade (in memory and `mars migrate`) moves Claude-only keys to `harness-overrides.claude`, the same list the Claude importer uses, so a pre-v1 skill's `argument-hint` still reaches `.claude/`.
- Commands that save `mars.toml` or `mars.local.toml` (`add`, `remove`, `link`, `unlink`, `override`, `init`, `models alias`, and sync mutations) edit the authored document: only the changed keys are touched, so comments, key order and unknown `[package]` and `[dependencies.<n>]` keys survive. Unknown dependency keys warn (`field.unknown`) like package keys. `mars add` then `mars remove` of the same dependency leaves `mars.toml` byte-identical. `mars init` writes `[dependencies]` only; v0.19.0 also wrote the default `[settings] models_cache_ttl_hours = 24`, which still applies when absent.
- Removing a frontmatter key no longer reorders the keys after it.
- `mars sync`, `check` and `doctor` warn once per pre-v1 package (`legacy-package`, with the count of files to upgrade) and no longer repeat each key the upgrade drops or moves; `mars migrate` lists those. The upgrade and `mars migrate` keep a CRLF file CRLF.
- A Claude-only key that `harness-overrides.claude` already sets is dropped with a warning that names it, and the override wins; a `harness-overrides` or `claude` entry that is not a mapping leaves the keys in place with a warning.
- Diagnostics keep the schema crate's own code (`yaml.non-json-value`, `skill.name-*`, `marketplace.*`) in `--json` output, not `schema-diagnostic`.
- A skill's `disallowedTools` is a removed key like `allowed-tools`: the reader errors on it and the pre-v1 upgrade drops it with a warning. It used to be an unknown field.
- "Valid harnesses" lists read claude, codex, pi, opencode, cursor.
- The `@meridian-flow/mars-schema` `.wasm` builds for size (936 KB, 373 KB gzip, from 1.27 MB). Parsing is about 60% slower: 78 definitions read in 7.9 ms, from 4.8 ms.
- `effort: max` is a level of its own, above `xhigh`, and no longer reads as `xhigh`: the bundle's `execution_policy.effort` carries `max` for every harness but Codex (a consumer that maps efforts must handle it; the bundle stays version 4), `mars build launch-bundle --effort` takes it, the `@meridian-flow/mars-schema` `Effort` type gains it, and a model alias's `default_effort` accepts it. Claude agent files and plugin trees write `xhigh` as `xhigh`; v0.19.0 wrote it as `max` because Claude Code had no other spelling. Codex tops out at `xhigh`, so a Codex file and a Codex bundle write `max` as `xhigh` and report it as approximately mapped. On Cursor, where the model slug carries the effort, `max` picks the `-max` variant, else the model's `-xhigh`, else `-high`, and warns.

### Fixed
- `mars sync` and `check` no longer walk `node_modules` and every other tree under a package looking for generated plugin output; one bounded walk checks the marker as it enters a directory.
- A skill with a YAML anchor reads its fields and reports `yaml.anchor` beside them, as an Agent does; `mars agents` and `mars skills` list it instead of skipping it.
- `mars check` prints its success line only when there are no errors.
- `mars remove <repository>` after `mars add <repository> --plugin <name>` (or a `--subpath`) says `did you mean` and names the dependencies that start with `<repository>/`; the dependency is named `<repository>/<plugin>`.
- Dependency `dialect` accepts `opencode` and the `open-code` spelling v0.19.0 required, and `agentskills`, `agent_skills`, `marsnative` and `mars_native` as the dialect parser does.
- A `mars.local.toml` `[settings]` key Mars does not read is kept, as in `mars.toml`, instead of failing the load; one that looks like network policy warns by name.
- A dependency with no `mars.toml` that carries keys only pre-v1 Mars defined (`fanout`, `models`, `model_invocable`, `approval: yolo`...) installs instead of failing as a foreign tree: those keys are rewritten one by one in whichever dialect the tree is read, so a `tools` list still restricts the agent and a sibling skill's `model_invocable` still renames, as in v0.19.0. A tree declared `dialect = "mars-native"` is upgraded as a pre-v1 package.
- A dependency with no `mars.toml` and an OpenCode Agent (a `permission` map, or a `tools` map of booleans) is read as OpenCode without a declaration: `write: false` becomes `write: deny` and `permission` and `temperature` reach `.opencode/agents/` through `harness-overrides.opencode`.
- A translation never moves a key Mars lowers itself (an Agent's `model`, `effort`, `tools`, `skills`, `mode`; a skill's `license`, `metadata`, `user-invocable`, `disable-model-invocation`, `alwaysApply`) under `harness-overrides`, where native lowering would discard it: an invalid entry of `tools` is dropped and the rest kept, an invalid scalar is dropped, each with a warning. Native lowering and the translation read one list per kind, both in `mars-schema`. The legacy upgrade drops what it cannot convert instead of writing it to `harness-overrides.flow`, which holds only `permission`.
- Losing a restriction in a translation or upgrade (a `deny` or `ask` tool entry, a `network-deny` host, `network`, `permission`, `sandbox`, `approval`) is a change coded `field.restriction-lost`, naming the agent. `mars sync` prints it on every run, beside the count of files translated, and `mars migrate` lists it with the other keys it dropped (plain output and `--json` `dropped`, for `.mars-src` too). For a `permission` map the note says it is OpenCode's and to declare `dialect = "open-code"`. A `network-allow` or `network-deny` list with one bad entry keeps its valid entries and drops only the bad one, reported per entry.
- Prediction: a skill's `allowed-tools` is a key the Agent Skills specification defines, so it no longer counts as Claude evidence, and Claude evidence found only on skills ranks below an Agent written the way only OpenCode writes one (a `permission` map). An OpenCode Agent beside a standard skill with `allowed-tools` reads as OpenCode, not Claude. A tree predicted OpenCode still reports `commands/` as not imported.
- OpenCode agent files carry deny rules as `permission: {<tool>: deny}` (OpenCode's current tool control; `tools` booleans are deprecated), merged over the author's `harness-overrides.opencode.permission`. A scoped deny rule, an allow and an `ask` stay reported as dropped. An OpenCode agent imported with `tools: {write: false}` keeps its denial in `.opencode/agents/`; the file used to lose it.
- `approval: yolo` in an Agent file is one error, not two.
- `mars doctor` hashes a bootstrap document as its directory, so it no longer fails with `Not a directory` (v0.19.0 did too).
- `mars check --root PATH` validates `PATH`; `--root` was ignored.
- `mars add <dir> --plugin <name>` (or `plugin = "<name>"`) on a Mars package says it is a Mars package, read as one, and to drop `plugin`. It claimed the `.claude-plugin/marketplace.json` was missing when a package's own plugin output had put one at its root.
- A Cursor plugin build no longer reports an Agent's `mode` as dropped (and `mars sync` no longer lists it as approximately mapped for `.cursor`): the Cursor agent file writes it.
- The "effort maps to --variant on subprocess only" note (OpenCode, Cursor) and Pi's effort note no longer fire for `effort: none`, which writes nothing for those harnesses.
- Colour codes are written only to a terminal: output piped or redirected carried raw escape codes unless `NO_COLOR` was set. `NO_COLOR` still turns colour off on a terminal.

### Security
- `mars sync` refuses a dependency holding a symlink that leaves the package (`authored package entry escapes package root`). v0.19.0 copied the target, such as `/etc/hostname`, into `.mars/`.
- A dependency URL, tag, ref or locked commit that starts with `-` is refused before git runs (`--upload-pack=<cmd>` would run `<cmd>`), and `--` precedes the repository and the checked-out revision. `plugin` names are letters, digits, `-`, `_` and `.`, starting with a letter or digit, so `mars add <marketplace> --plugin ..` is an error. `mars-schema` refuses a marketplace entry's unsafe repository or ref (option-like, whitespace, `ext::`, `file://`, an absolute path, any scheme but `https://` and `ssh://`) as an `unsupported` source with a reason, and a `sha` that is not a full commit; a plugin name that is not a name is left out of the list. A marketplace can no longer name a local repository for a plugin.

## [0.19.0] - 2026-10-26

### Added
- `mars build plugin [PATH] [--out <dir>]` builds owned Claude, Cursor, and OpenCode plugin trees
  from a package's publish graph. Explicit output uses the nested
  `<dir>/{claude,cursor,opencode}/` layout with a root `.mars-plugin.json` sentinel.
- `[plugin]` configures committed output, identity, marketplace, description, and harnesses.
  Configured builds also generate the Claude and Cursor catalogs and root OpenCode `package.json`;
  `[package]` supplies author, homepage, and license metadata.
- `mars check` reports stale, hand-edited, skipped, or root-conflicted configured plugin output and
  warns when plugin packages do not require Mars 0.19.0 or newer.
- Build and commit configured multi-harness plugin output during `mars version`, with rollback-safe
  publication and a `--no-plugins` escape hatch.
- Plugin builds refuse source symlinks that leave the package, and special files, so published
  output never carries files from outside it. `[package].authors` takes `"Name <email>"` strings or
  `{ name, email }` tables, and plugin-only metadata never blocks `mars sync` or `mars list`.

### Fixed
- `mars sync --diff` and `mars version`'s validation pass no longer create `.mars/` state: a dry
  run stages in a temporary directory and never creates a sync lock (it takes an existing one so a
  concurrent real sync cannot show it a torn snapshot), so a package repo that does not gitignore
  `.mars/` stays clean for the next release.
- OpenCode sync now registers MCP servers under native `mcp` entries with `{env:VAR}`
  environment references and records each written entry in `mars.lock`. Legacy `mcpServers`
  entries Mars owns migrate on the next sync, and stale cleanup removes only the section the lock
  names, so same-named user entries survive. A non-object `mcp` value fails sync before any write.
- A failed MCP config write keeps the lock records of unchanged entries, which are still on disk,
  so a later sync can still remove them. It used to drop every record for that target.
- OpenCode sync now warns when a hook fragment lacks the required default `{ id, setup }` export,
  matching plugin builds while still installing the fragment.
- Plugin-only package metadata no longer blocks `mars version` for packages without `[plugin]`,
  and malformed non-table `plugin` values survive config loading and saving so non-plugin commands
  can continue while `mars check` and plugin builds report the invalid value.
- Native agent compilation now preserves `skills.load` versus `skills.available`: Claude and
  Cursor no longer preload available-only skills, while every target receives `/name` body
  guidance for skills its per-agent format cannot preload. Claude also repeats load guidance as a
  reminder for `--agent`, whose session path does not honor the native preload field.
- Hook order now follows shortest dependency depth, changing only when a direct dependency is
  also reached transitively. Codex users may need to re-trust moved hooks once with `/hooks`.
- Discovery, MCP discovery and staging skip any directory holding a valid `.mars-plugin.json`
  (generated plugin output), so a committed plugin tree is never re-imported as package source.
  A package's own configured `[plugin].dir` is excluded directly and silently from its `sync` and
  `check`; other generated trees still report each skipped path.
- Explicitly or automatically renamed agents and skills now carry their installed names in
  frontmatter, and `subagents:` references follow explicit agent renames as well as collision
  renames. Native agent sync also removes prior claims that are absent from the current emission
  set, including outputs left by dependency removals and renames.
- MCP server collisions now prefer the shallowest package, then dependency declaration order,
  so a package's own server cannot lose to a transitive dependency.
- Skill and hook copies now preserve source executable bits.

## [0.18.4] - 2026-10-25

### Changed
- Agent inventory prompts no longer include Meridian prompt-file and parallel-spawn usage tips;
  Meridian's injected guidance owns command usage.
  
## [0.18.3] - 2026-10-25

### Fixed
- Unknown hook and MCP `visibility` values now warn and remain local instead of being silently
  treated as local.

## [0.18.2] - 2026-10-25

### Fixed
- `mars check PATH` now resolves package path dependencies relative to the checked package,
  rather than the caller's working directory.
- `mars check` and `mars init` now report publish-view configuration warnings instead of
  dropping them; `mars check` also reports dependency-resolution warnings.
- Hook emission now uses the same filter- and collision-selected hook as installation, so an
  excluded hook no longer suppresses another dependency's installed hook or causes a false
  not-installed warning; shadowed dependency hooks no longer cause that warning either.
- Hook fragments removed by a filter or shadowed by a local hook are no longer validated, so
  a broken one no longer fails `mars sync`.
- `mars version` now changes only `[package].version`, preserving comments, formatting,
  ordering, and unrecognized tables in `mars.toml`.

## [0.18.1] - 2026-10-24

### Changed
- `mars version` now fills an empty changelog release section from package commit subjects since the latest reachable stable `v*` tag, opens an editor only with terminal stdin/stdout/stderr, and restores the starting commit plus changelog and config state when a pre-tag release step fails.

## [0.18.0] - 2026-10-24

### Added
- Agent `network: allow|deny|allowlist` mode plus canonical `network-allow` and
  `network-deny` host sets. Modes resolve by policy precedence; host sets merge across user
  config, project/local settings, profile, overlay, and repeatable CLI flags. Hosts accept
  exact names, IPs, `*.x` (subdomains), and `**.x` (apex plus subdomains). The launch bundle
  adds `execution_policy.network` as one `{mode, allow, deny}` object plus
  `provenance.network_source`, and fails an empty allowlist closed to `deny` with a warning.
  Both `_` / `-` separator spellings are accepted, duplicate spellings are errors, and other
  network-looking keys warn instead of silently losing a deny. Host validation rejects ambiguous
  numeric and hex-style endings and top-level wildcards, canonicalizes IPv4-mapped IPv6, and
  points Unicode users to punycode. Built-in web tools remain governed by `tools: web`; harness
  launchers report enforcement limits.
- User network defaults in `$MARS_CONFIG_DIR/config.toml` (or the platform Mars config
  directory): `[settings] default_network`, `network-allow`, and `network-deny`. An empty
  `MARS_CONFIG_DIR` is treated as unset.
- `--network allow|deny|allowlist` plus repeatable `--network-allow` / `--network-deny`.
  Package profiles reject deny-host replacement and tool replacement tables; those replacement
  forms belong to consumer overlays.

### Changed
- **Upgrade:** if a plain `[agents.<name>.tools]` or `[skills.<name>.tools]` `allowed` /
  `disallowed` list was intended to narrow the profile, change it to `{ replace = [...] }`
  ([how lists combine](docs/config/mars-toml.md#adding-to-or-replacing-a-permission-list)).
  These are the only existing permission sets whose behavior changes; the network sets below
  are new. Non-empty plain tool lists now add, while an empty plain list remains a no-op. A
  replacement drops every lower rule for that action, including `'*': allow`; existing `ask`
  rules and the `'*': deny` allowlist marker survive. For one release, Mars warns once for each
  non-empty plain tool list and names its source file; empty lists and `{ replace = [...] }` do
  not warn. A project tool list also now applies when a matching local block omits that list; the
  warning names both files and recommends a local `{ replace = [...] }` list or removing the
  project list. Package profiles cannot use replacement tables. The launch bundle, native agent
  files, and staged skills all use the same merge.
- **Upgrade:** matching `mars.toml` / `mars.local.toml` agent and skill blocks still use 0.17's
  whole-entry replacement for scalars, options, model policies, and unknown keys. The two new
  network sets (`network-allow`, `network-deny`) and the two changed tool sets (`tools.allowed`,
  `tools.disallowed`) instead compose. Use `{ replace = [...] }` where a local set should discard
  the project set and lower profile entries. Adding only `network-deny` to a local agent block
  still drops the project block's `model`, `harness`, `description`, and model policies; repeat
  the needed project scalars and model policies in the local block. `[models.*]` retains
  whole-entry replacement.

## [0.17.0] - 2026-10-22

### Added
- `image` tool capability (alias `view_image`): Codex `view_image`; Claude, OpenCode and Pi open images with their read tool, so it has no switch there.
- Launch bundle `tools.capability_natives` (additive in bundle schema 4), always carrying the
  resolved harness's native names and status for the `agent` and `task` capabilities. Launchers
  can enforce platform rules without keeping a second native-name table; the names come from
  the same projection as `tools.rules[]` and retain older-build names such as `TaskOutput`.
- Launch bundle `tools.rules[]` (additive in bundle schema 4): one entry per profile
  statement, `{capability, pattern, action, native, native_tools, native_status}`.
  `native` carries permission-rule spellings; `native_tools` carries the unscoped harness tool
  names the rule concerns. Consumers no longer have to infer either fact from the other.
  `native_status` is `known`, `absent` (the harness has no such tool) or
  `unknown` (Mars cannot name it: a gap to report, with the reason in `note`, for example
  "codex loads skills by reading files; no switch"). The `*` rule names no tool: empty
  `native` and `native_tools`, no `native_status`, no `note`. `tools.allowed` / `tools.disallowed` /
  `tools.mcp` stay, derived from the rules, for one release; they cannot carry `ask`, `*`
  or the capability behind a name.

### Changed
- Clarify bundle MCP permissions and document Meridian's compaction and skill-description use.
- `.cursor` target unmaintained. Support stays; no longer tested or extended.
- Tool vocabulary: `task` now means the task-list tools (Claude `TaskCreate`…,
  OpenCode `todowrite`, Codex `update_plan`), no longer an alias of `agent`
  (sub-agents). `web` is its own capability (`web_search` + `web_fetch`), no longer an
  alias of `web_search`. `todo_read`/`todo_write` fold into `task`. Claude-dialect
  packages keep working: staging renames Claude's `Task` tool to `agent`. That lift runs
  for every package not declared `dialect = "mars-native"`, in list and map form, and only
  for the exact spelling `Task`; every other name is case-insensitive. So `Task` means
  sub-agents in a lifted package and the task list in a mars-native one: set
  `dialect = "mars-native"` on the dependency whose `Task` means the task list.
- `tools: allow` / `tools: deny` (scalar) is a validation error instead of being read as
  a tool named `allow`/`deny`.
- `tools:` maps accept `ask` as an action next to `allow` and `deny`, and `'*'` as a
  capability (`'*': deny` plus allows is an explicit allowlist). Native agent files
  cannot hold `ask`, or an allowlist with nothing allowed; both are reported as
  dropped lossiness.
- Native tool names come from explicit per-harness tables (`compiler/tool_natives.rs`),
  one capability to one or more real tools: bare Claude `edit` is `Edit`, `Write` and
  `NotebookEdit`; `web` is `WebSearch` + `WebFetch` (OpenCode `websearch` + `webfetch`);
  `ask_user` is `AskUserQuestion` (OpenCode `question`). A capability a harness cannot do
  is `absent`; one it can do with no tool to switch it (Codex and Pi `skill`, Codex
  `web_fetch`, and Codex `read` / `glob` / `grep`, which it does through the shell) is
  `unknown`, so an unenforced deny is reported instead of counted as met; that gap carries
  its reason in `note`. A name Mars has no row for, or does not recognize, is left out
  instead of guessed, with its reason in `note` and, at `mars sync`, a warning (the
  PascalCase / lowercase naming convention is gone). The launch bundle adds no
  `warnings[]` entry for any permission gap: `native_status` and `note` are the only
  channel. Claude `task` also names `TaskOutput`, which Claude removed in
  2.1.277 but older builds still offer. Codex `bash` covers `exec_command` alone
  (`write_stdin` only feeds a session `exec_command` opened); `agent` covers the whole
  sub-agent tool family on Claude (`Agent`, `SendMessage`, `ListAgents`) and Codex
  (both multi-agent versions). Fixes Claude `Websearch` / `Askuser`, and OpenCode
  `browser`, which matched no tool. Cursor rows are carried over unchanged.
- Claude scoped file rules now use the permission categories Claude actually consults:
  `write(path)`, `edit(path)` and `notebook(path)` emit `Edit(path)`, while `read(path)`,
  `glob(path)` and `grep(path)` emit `Read(path)`. Bare rules keep their tool names, and
  `WebFetch(domain:...)` was already correct. Their `native_tools` still name the actual
  callable tools (`write(path)` → `Write`, `grep(path)` → `Grep`), so an allowlist does not
  remove the tool while adding its scoped permission. Known scoped rules carry a `note` when
  the category covers sibling tools; when two scoped capabilities collapse to one permission,
  `deny` beats `ask`, which beats `allow`.
- A rule on a narrower capability wins over a broader one for the tools they share, so
  `edit: deny` with `write: allow` leaves Claude's `Write` allowed (the `edit` deny covers
  `Edit` and `NotebookEdit`) instead of denying it. The bundle's `native` lists, the flat
  lists and native agent files carry the resolved names. A tool that stays both allowed
  and denied because the harness has one tool for both capabilities (Codex and OpenCode
  `write` / `edit`) is reported as agent-file lossiness and as a `note` on both bundle
  rules, with no bundle warning.
- A Claude agent file gets `tools:` only from a map-form `'*': deny` allowlist, because
  Claude reads that field as "only these tools". Map-form `allow`
  entries are grants, not an allowlist, so they no longer turn into `tools:`. Omitting
  `tools:` inherits every tool, so nothing is lost and no lossiness is recorded.
- `mars.toml` `[agents.<name>.tools].allowed` replaces the profile's allow rules and keeps
  its deny and `ask` rules; `disallowed` replaces its deny rules. The launch bundle now
  applies the overlay too, through the same merge as native agent files. `[skills.<name>.tools]`
  uses the same edit, so `allowed` no longer drops a map-form deny or `ask` in the skill's
  `tools:`; `allowed` only grants and makes no allowlist. The staged skill writes
  entries in canonical spelling (`agent`, not `Agent`), and denies in `disallowed-tools:`.
- A scalar `tools: Read, Grep` (Claude's own agent-file spelling) is read as the list
  `[Read, Grep]`; staging splits it before lifting `Task` and `mcp__…` entries.
- A malformed `tool(...)` entry (unclosed or empty parentheses, `*(x)`) is a validation
  error instead of passing through verbatim to the harness. So is a bare `mcp` or `MCP`,
  which names no server: it was an unknown tool that collided with the bundle's `mcp`
  capability.
- `bash` patterns `X:*` normalize to `X *` (`bash(git revert:*)` → `bash(git revert *)`),
  the one spelling OpenCode reads correctly. Other capabilities keep the colon, since it is
  part of a path or host (`read(./secrets:*)`, `web_fetch(domain:*)`).
- `mars agents show --json` / `mars skills show --json` print the tool policy as one `rules`
  list, in the launch bundle's rule shape (`{capability, pattern, action}`).
  `mcp(server)` shows as `mcp(server/*)`. The text view prints one `tool-rules` line.
  **Deprecated:** `tools` and `disallowed-tools` stay for one release, derived from `rules`
  (they cannot carry `ask`, `*` or the capability behind a name); `tools-denied` is folded
  into `disallowed-tools`.
- Tool names in `mars.toml` / `mars.local.toml` `[agents.<name>.tools]` and
  `[skills.<name>.tools]` are checked when the config loads, with the profile grammar: a
  malformed name (`mcp`, `mcp(`, `*(x)`) fails the load instead of reaching the policy. A skill
  overlay leaves a skill with a malformed `tools:` / `disallowed-tools:` entry untouched, so
  sync reports the entry.

### Deprecated
- List-form `tools:`. Still only grants. Warns. Use map form; add `'*': deny` for only
  those tools.

## [0.16.0] - 2026-10-14

### Added
- `--fallback-root` layers missing agents and skills from a secondary Mars
  project for launch bundles and the `agents` / `skills` inspection commands,
  while preserving the primary root as the sole configuration authority.
- Launch bundles and agent / skill inspection JSON expose absolute content
  source paths; layered list entries also identify their `root` or `fallback`
  scope.

### Changed
- `agents show` and `skills show` also match content by its installed file or
  directory stem, not only by the name declared in frontmatter.

## [0.15.0] - 2026-09-28

### Breaking
- `mars models list` is now the curated harness×model display, not the alias
  inventory; `--all` bypasses curation, not a raw catalog switch. The former
  `--include`, `--exclude`, `--providers`, `--no-visibility`, `--catalog`, and
  `--unavailable` flags and project/local `[settings.model_visibility]` are
  removed. Move authored display rules to `mars.curated.toml` or
  `mars.curated.local.toml`. Dependency package consumer settings are ignored
  when reading manifests and do not block sync.
- Meridian P4 migrates `models list --json` to `models aliases --json` and
  `models list --all --json` to `models catalog --json`. Publish Mars first;
  existing Meridian releases remain on their exact older Mars pin. Then update
  Meridian's exact pin and lock to the published package, verify the installed
  binary, and release Meridian. `PATH` overrides of bundled Mars require a
  coordinated upgrade.

### Added
- Split `mars models` into curated harness-model `list`, uncurated `aliases`,
  and raw `catalog` commands. `list --all` includes hidden rows; `--live` adds
  fixed-harness eligibility. JSON includes full Possible provenance and curation.
- Add a non-persisted Possible harness-model projection from the models.dev
  catalog and retained Pi/Cursor/OpenCode listings, including listing provenance.
- Add strict, display-only user/project/local curated TOML rule loading and
  deterministic tier evaluation for the `mars models list` command.

### Changed
- Treat the models.dev cache as indefinite last-known-good data. The 24-hour
  setting is refresh-after: stale usable catalog reads return immediately and
  refresh in a detached, deduplicated worker. Cold and forced refreshes still
  block; offline modes use disk only. Failures retain data and apply backoff;
  catalog JSON exposes stale/background status.
- Coalesce concurrent stale catalog readers at worker launch, not just at the
  network fetch. Recover crashed refresh claims after a bounded lease, reap
  POSIX workers in long-lived callers, and refresh project-less ad-hoc launch caches.
- On Windows, detach stale-catalog workers without inheriting captured output
  handles, so catalog commands return before background network work finishes.
- Bound catalog HTTP work end-to-end below the refresh-claim lease, including
  DNS and redirects. Recheck snapshot revision, cache freshness, and failure cooldown
  under the claim lock so paused readers neither launch after a peer completes
  nor bypass a peer's cooldown.
- Store catalog data, fetch time, and revision in one atomic snapshot; legacy
  generation sidecars are ignored. Centralize detached catalog and probe worker
  launches behind one no-inherited-handles platform API. Keep cache lifecycle
  methods together and test fixtures separate from the production source.
- Surface one top-level diagnostic per retained harness listing whose latest
  refresh failed, even when curation hides every row; distinguish this from a
  cold unavailable listing. Unknown prompting refs now point to alias inventory.
- Canonical `opencode` harness/probe JSON names, cold listing-failure diagnostics,
  exact per-provider-variant live verdicts, and stable nullable live fields in
  list JSON. Static aliases JSON no longer asserts an unavailable harness source.
- Remove `settings.model_visibility` and its old `models list` flags. Project
  and local legacy tables now fail with file-named curation migration examples.
- Preserve failed cold probe errors in command-local observation snapshots.
- Project runnable provider and launch ID together from the selected route;
  use typed harness IDs for Cursor build policy.
- Canonicalize provider-qualified Claude/Codex curation literals to native
  model IDs, preventing phantom declared rows. Precompute literal match context
  for catalog-sized lists; retain probe provenance atomically with outcomes.
- Treat an empty successful Cursor model listing as an empty Possible inventory
  without implying authentication. Normalize native provider labels, exclude
  curated files from flat skill installs, and avoid duplicate scope diagnostics
  or default warnings for uninstalled declarations.
- Honor supported authored harness preferences even when native auth is inconclusive
  or an ungated listing leaves auth unchecked; stop model fallback at that route.
- Treat successful Pi/Cursor credential-gated listings as configured-auth evidence,
  while OpenCode remains unchecked. Failed later listings retain last-good model
  support but lose implied auth until refresh succeeds.
- Derive live model availability and launch IDs from the selected routing assessment
  instead of independently rechecking support. Native launch IDs preserve the
  requested punctuation/case, while probe-backed IDs use the selected slug.
- Treat a cold failed Pi model listing as support-unknown passthrough, not a
  negative model match. Such an unverified route may launch when no better
  candidate exists; `models resolve` now says available listings cannot confirm
  it rather than claiming a model mismatch. Share typed listing evidence across
  routing consumers.
- Include `selected_by_preference` in route decision JSON (report version 3).

## [0.14.4] - 2026-09-23

## [0.14.3] - 2026-09-23

### Added
- `settings.model_visibility.providers` filters `mars models list` to the declared
  providers (exact, case-insensitive, variant-collapsing). Display-only: a hidden
  alias still resolves when named, and a plain model string still passes through.
  Empty and blank entries are ignored, so `providers = []` behaves like unset, and
  aliases that resolve to the `unknown` provider are never shown while the filter
  is active.
- `mars models list --providers` overrides config; `--no-visibility` shows every
  alias regardless of config or flags.

### Changed
- `settings.model_visibility` ignores blank entries in `include`, `exclude`, and
  `providers`, so a blank-only list behaves like unset for all three. Previously a
  blank `include`/`exclude` entry was a literal pattern that matched nothing.

## [0.14.2] - 2026-09-22

### Fixed
- Allow `mars sync --force` to explicitly adopt unowned regular self agent files and skill directories, recording normal `_self` ownership while continuing to reject symlink destinations.

- OpenCode native agents now emit the provider-qualified model route selected by the OpenCode probe instead of a bare canonical model id.
- Native-emission coverage now verifies provider preference, qualified inputs, and unavailable-route fallback.
- Explicit provider-qualified OpenCode model literals now remain on their requested provider when variants share a model id.

## [0.14.1] - 2026-09-17

### Fixed
- Honor alias `provider` when resolving OpenCode/Pi `harness_model`. A pin such as
  `xai` no longer drops its filter and pick OpenCode Zen (`opencode-go/…`) over
  `xai/…`. Broad `openai` pins still prefer `openai-codex` variants.

## [0.14.0] - 2026-09-16

### Added
- Literal launch-model pins bypass aliases and preserve deliberate harness defaults; expose provider constraints for continuation replay.

### Changed
- Launch bundle schema 4 and routing report schema 2 retain target provenance and every attempted model; JSON failures carry structured errors and available selection history.

### Fixed
- Make bundle and alias checks independent of installed developer harnesses and native log line endings.
- Preserve named Pi models and unresolved-alias provider constraints in final executable-model projection.
- Route standalone aliases through harness preferences; preserve base-alias provider constraints on prefix resolution.
- Resolve provider-qualified pinned aliases without deriving provider identity from their preferred harness.
- Treat launch profile/overlay/alias harnesses as preferences; preserve independent CLI pins and remove model-clearing recovery.
- Prefer an eligible backup over unverified primary or earlier backups; retain the first unverified attempt only when no model has an eligible route.
- Preserve unknown native auth as unverified; check auth for harness-default launches and cache observations per command.
- Separate runtime eligibility from model support; prefer eligible routes over unverified support and reject blocked fixed assessments.
- Mark rejected live model routes unavailable and prune them; never advertise unassessed harness paths as runnable.
- Remove unchecked default/linked route fallback and no-binary route fabrication; assess each permitted harness once.
- Treat harness order as preference, trying remaining permitted harnesses through the same evaluator.
- Apply caller harness exclusions before route selection and probes; explicit pins cannot override them.
- Enforce target scope for launches and standalone model commands; preserve empty scope and winning-file provenance.
- Remove unrestricted alias pre-routing and excluded availability paths; fail model resolution when no route is selected.
- Scan all concrete profile model-policy backups regardless of the active settings match; `no-fallback` excludes only its entry.
- Share fallback enumeration with inventory and distinguish literal model IDs from alias labels.

## [0.13.2] - 2026-09-16

### Changed
- Prune test scaffolding, duplicate assertions and forwarding wrappers. Fix weak safety checks; isolate touched CLI fixtures. Keep ownership and recovery regressions.

### Fixed
- Keep native agents present on the first successful retry after an interrupted destination move. Obsolete paths no longer erase surviving ownership.
- Recover new canonical outputs after failed or interrupted sync using durable write intent. Repeated updates and destination moves retain ownership until finalization.
- Preserve corrupt lock evidence through failed repair. Refuse changed outputs, links, malformed recovery records, and frozen recovery; no-op lock mtimes stay unchanged.
- Reserve the recovery journal path, including portable case/dot/space aliases, so custom destinations cannot overwrite or delete recovery evidence.
- Keep flat `.mars-src` resources whose names resemble project output directories; exclusions follow the actual source root.
- Flat self skills exclude native output trees including previously owned custom targets after configuration changes or when configured with absolute/dot-segment paths.
- Document self-sync recovery and occupied-layer discovery, including nested fallback and the non-hidden output limitation.
- Blocked self destinations fail before output writes, including identical bytes and `--force`. Unchanged-content source transitions refresh ownership without losing native output claims.
- Declared packages sync their own agents and skills as Mars-native sources. `.mars-src` wins before staging; dependency renames stay intact. Flat-root skills keep resources without copying generated output trees.

## [0.13.1] - 2026-09-11

### Added
- `[settings] catalog_providers` replaces the models.dev ingest allowlist. `["*"]` keeps every provider.

### Changed
- Default catalog ingest is now `anthropic`, `openai`, `google`, `meta`, `deepseek`, `xai`, `openrouter`. Mistral and Cohere are no longer ingested unless listed.

## [0.13.0] - 2026-07-29

### Added
- Package manifests can declare strict `requires-mars` and
  `requires-meridian` engine version constraints. Sync, upgrade, add, and
  repair enforce them with explicit ignore flags; `mars check` validates their
  syntax, and JSON sync reports describe engine-driven version fallbacks. A
  present but malformed `MERIDIAN_VERSION` fails closed when a package
  declares `requires-meridian`; engine remediation hints are
  direction-neutral.

## [0.12.1] - 2026-07-28

## [0.12.0] - 2026-07-25

### Changed
- **Breaking:** `mars.lock` version 3 distinguishes installed outputs from
  pending-deletion ownership records. Version 2 locks are promoted for one release
  so legacy hook cleanup can run; version 1 locks must be removed and regenerated.
- **Breaking:** `mars unlink` now removes only outputs and config entries recorded
  as owned for that target; handwritten files remain in place, and the target
  directory remains when it is not empty.
- Leave byte-identical native agents, native skill projections, `mars.lock`, and
  `.mars/native-agents.json` untouched during a no-op sync, preserving mtimes
  and avoiding redundant durable writes.
- **Breaking:** remove unused public helpers, wrappers, re-exports, and unread
  fields from the library API.
- `discover_installed` now scans only installed item identity and path instead of
  parsing every installed agent and skill frontmatter document.
- `mars doctor` conflict detection now matches `mars list --status` and `mars resolve`.
- Stop creating the unused `.mars/cache/bases` directory during sync.
- **Breaking:** hooks now author per-target native JSON fragments instead of
  `events`/`matcher`/`[action]`; Mars installs whole hook directories and
  substitutes `${MARS_HOOK_DIR}` with portable path separators. The old schema
  is rejected as an error.
- The v0.11.0 command-path cleanup joins the #130 one-release sweep deletion
  ledger.
- Hook discovery now consistently treats only package-root `hooks/` as the
  hook convention root.
- Same-name hook collisions are scoped per target.

### Added
- Cursor native hook fragments emit flat entries under a Mars-owned version 1
  wrapper with strict validation against its 21 camelCase events.
- OpenCode and Pi hooks place substituted TypeScript file fragments at
  `plugins/mars-<name>.ts` and `extensions/mars-<name>.ts`, respectively.

### Fixed
- Guide consumers to use `upgrade --bump` when removed hook schemas remain
  behind a package version constraint.
- Preserve corrupt `mars.lock` bytes when repair halts on an unreadable legacy
  hook source; the lock is now rebuilt in memory and replaced only after a
  complete repair.
- Let `upgrade`, `override`, and `remove` recover projects whose resolved
  packages still contain the removed hook schema. When legacy sources remain,
  recovery persists only the requested config intent and exits 2 before
  materialization with blocker guidance; `repair` likewise halts without
  writing. A fully readable post-mutation graph converges in the same command.
  Normal sync, including `sync --force`, continues to reject the old schema.
- Apply local overrides to transitive dependencies, allowing a broken
  transitive package to be replaced without first declaring it directly.
- Reject conflicting same-name transitive source declarations even when a
  project override supplies one effective replacement path.
- Preserve installed-content authority for matching skill and hook directories
  when promoting version 2 locks.
- Reject version 2 directory outputs containing symlinks or special entries
  before promotion can grant installed-content authority.
- Repair linked native skill projections when the destination is a symlink,
  a non-directory, or has structural differences such as extra empty directories.
- Describe pending-deletion copy collisions as paths without installed-content
  claims instead of incorrectly calling them untracked or divergent.
- Preflight `mars adopt` before moving user content, and restore the original
  path if the subsequent sync fails.
- Report stale config entries as removed only when removal was confirmed for
  that entry's MCP or hook surface.
- Record exact emitted hook config and remove only structural matches, preserving
  hand edits and reporting them as config divergence.
- Preserve untracked and hand-edited linked-target files; require `--force` to
  adopt collisions.
- Preserve ownership and suppress replacement after a failed removal so a later
  sync retries the removal.
- Treat pending-deletion records as deletion authority only, preserving content
  at a reintroduced path unless `--force` explicitly adopts it.
- Allow OpenCode file hooks to sync without validating an unrelated
  `opencode.json`.
- Make transitive hook precedence use deterministic longest-path depth, avoiding
  hook config churn across identical syncs.
- Reject malformed merge config before writing hook files instead of replacing
  user config.
- Skip MCP discovery for dependency filters that cannot emit MCP config.
- Report removed hook schemas from filtered transitive packages with source
  name and version instead of leaking internal staging paths.

## [0.11.0] - 2026-07-24

### Changed
- **Breaking:** hooks now declare harness-native events in per-target
  `[targets."<target>"]` tables. Migrate `event =` plus `targets = [...]` to
  `events = ["<native event>"]` under each target table.
- Codex `SessionEnd` hooks are rejected because the upstream-documented event
  is non-functional in codex-cli 0.144.4; use `unchecked = true` to override.

### Fixed
- Stop writing fabricated OpenCode `hooks` config. A removal-only sweep remains
  for one release to clean entries written by earlier Mars versions.
- Sweep stale hook bindings before writing native replacements so one sync
  converges upgraded config and lock state.
- Remove path-matched managed hook residue from legacy Codex string bindings
  in `codex_hooks.json` for one release while preserving user-authored commands.
- Prune Codex hook event keys emptied by managed-binding cleanup while
  preserving pre-existing user-authored empty arrays.

## [0.10.6] - 2026-07-24

### Fixed
- Claude Code hooks on `session.end` now compile to the valid `SessionEnd`
  event, so the hooks actually fire when a session ends.

## [0.10.5] - 2026-07-10

### Fixed
- npm publishing stays on npm 11.18.0 until npm 12 restores the missing
  `sigstore` provenance dependency, preventing release jobs from failing before
  the first platform package is published.

## [0.10.4] - 2026-07-10

### Fixed
- Removing a rename rewrite now reinstalls the raw source instead of leaving
  stale rewritten frontmatter on disk.

### Changed
- Restore auto-rename for cross-source naming collisions — both colliders are
  suffixed with `__{source_name}` instead of failing with a hard error.
  Explicit `rename` in mars.toml still takes priority.
- Rename rewrites now run once after unmanaged collision pruning and choose
  dependency candidates in mars.toml declaration order.

### Added
- `config-rename-dangle` warning when a config reference
  (`[settings.meridian.fanout].agents`, `[agents.<name>]` overlays, or
  `[skills.<name>]` overlays) points at an agent or skill name that an
  explicit or collision rename has replaced, so a silently mismatched
  config no longer goes undiagnosed.

## [0.10.3] - 2026-07-06

### Fixed
- Codex hook lowering now writes current `hooks.json` command-hook bindings with native Codex event names.

## [0.10.2] - 2026-06-25

### Fixed
- Inbound lift now rewrites foreign MCP tokens in **map-form** `tools:` keys (e.g. Claude `{ mcp__github__delete_repo: deny }`, Cursor `{ Mcp(github:delete_repo): deny }`). Previously only string/sequence tool values were lifted, so map-form MCP keys leaked foreign wire tokens into canonical `.mars/` unchanged or were reparsed as whole-server refs — silently changing per-tool allow/deny semantics. Non-MCP keys and each entry's allow/deny value are preserved.

## [0.10.1] - 2026-06-25

### Fixed
- OpenCode skills no longer emit a per-skill `model-invocable` dropped-field warning. OpenCode's skill frontmatter has no model-invocation gate, so the axis is structurally unrepresentable; the drop is uniform across every non-model-invocable skill and not actionable by the author. It is now an accepted known target limitation (silent drop) rather than per-item lossiness noise. Genuine OpenCode losses (tool gating, `user-invocable`) still warn.

## [0.10.1-rc.1] - 2026-06-24

### Fixed
- Cursor skill `model-invocable` now maps to native rule modes — `true`/default → Apply Intelligently (`alwaysApply: false` + `description`), explicit `false` → Manual (`alwaysApply: false`, no `description`) — instead of the old `alwaysApply: true` (which forced always-on context) and silent drop on `false`.
- Codex skills with explicit `model-invocable: true` no longer emit a spurious dropped-field warning — Codex is model-invocable by default, so explicit-true loses nothing.
- Lossiness warnings are tiered by behavioral consequence: launch-time/meridian-enforced agent fields (`approval`, `sandbox`, `mode`, `model-policies`, `autocompact`, `fanout`, …) are no longer warned per-agent — they collapse into one summary line, since Meridian applies them at spawn. Genuine target-enforced losses stay loud. `--verbose` restores full per-item detail.
- Inbound lift no longer infers `user-invocable: false` from an OpenCode agent's `mode` (the axes are independent now that `mode` lowers exactly).
- Inbound lift round-trips Cursor Manual skills: an `alwaysApply: false` rule with no `description` restores canonical `model-invocable: false` (and re-emits the Codex `openai.yaml` sibling) instead of erroring.

### Added
- Codex `openai.yaml` sibling (`policy.allow_implicit_invocation: false`) emitted for skills with explicit `model-invocable: false` — closes the Codex skill-invocation gating gap (#116). `LoweredOutput` now carries `siblings` written atomically alongside the primary artifact.
- OpenCode agents emit native `mode: primary|subagent` instead of approximate lossiness.
- `when_to_use` folds into native `description` for Cursor / OpenCode / Codex (which use `description` as the selection hook) instead of being dropped.
- `LossinessMode::Verbose` plus a `--verbose` flag on `sync` / `check` to surface meridian-only/launch-time field detail.

### Changed
- Skill `description` is required only at authored-source validation (MarsNative staging + `mars check`); canonical re-parse tolerates an absent description so lifted Cursor Manual imports stay valid without a fabricated description.
- **Breaking (Cursor output):** Cursor skill `.mdc` frontmatter changes (`alwaysApply`/`description` from the rule-mode + `when_to_use` mapping). Existing Cursor projections are reprojected once on next sync.

## [0.10.0] - 2026-06-24

### Fixed
- Source discovery finds convention `agents/` / `skills/` / `bootstrap/` directories nested below the package root, not only at the top level.
- Duplicate `(kind, name)` discoveries — across convention directories, or a convention item vs a `.claude-plugin` manifest declaration — now raise `DiscoveryCollision` instead of silently keeping one by path order.
- MCP inbound lift: merge `allowed-tools` with `mcpServers` without dropping the allowlist; preserve map-form `tools:` when appending whole-server `mcp(server)` entries.
- Malformed `mcp(...)` tool tokens (e.g. `mcp()`, `mcp(/x)`) are validation errors instead of convention-projected unknown tools — disallowed malformed MCP refs no longer fail open.
- Agent parser: retired top-level `mcp-tools:` / `mcp_tools` emits `RemovedField` diagnostic and staging strips the field from canonical frontmatter (mirrors skills).
- MCP inbound lift: boundary-safe Claude `mcp__` wire parse (no UTF-8 panic on near-prefix tokens); Cursor `Mcp(server:tool)` uses the last `:` as the tool separator so namespaced server ids lift to per-tool `mcp(server/tool)` instead of whole-server; scalar `allowed-tools` / `disallowedTools` strings lift like list entries.
- No-op `mars sync` no longer rewrites linked native skill outputs when projected bytes are already on disk (mtime churn on `.claude/skills/.../SKILL.md` etc.).
- Hook lossiness (`hook-dropped`/`hook-approximate`) now follows the same surface/suppress policy as agent/skill lossiness — hidden on `validate`/`export`/`add` and no longer escalated into a failure by `mars validate --strict`.
- `skill`, `workflow`, and `web` (alias of `web_search`) are recognized tool names — gating them (e.g. `skill(deep-research)`, `Workflow`) no longer emits "not a known tool; passing through verbatim" warnings and projects to native spellings (`Skill(...)`, `Workflow`, `WebSearch`). Recognition only — per-harness enforcement of `disallowed-tools` is unchanged.
- Unknown tools now project via the target harness naming convention instead of passing through verbatim (snake_case custom tools reach Claude/Cursor as PascalCase); MCP wire names (`mcp__…`) are recognized and preserved verbatim without re-casing.

### Added
- Inbound lift of foreign MCP permission tokens to canonical `mcp(server/tool)` on import — Claude `mcp__server__tool` wire refs and Cursor `Mcp(server:tool)` refs in `allowed-tools` / `disallowed-tools` lift without case change; `mcpServers` whole-server entries lift into `tools:` as `mcp(server)`.
- Canonical `mcp(server/tool)` grammar in `tools:` / `disallowed-tools:` — scoped MCP references are parsed and recognized (preserved verbatim, no convention projection or unknown-tool warning); real per-harness emission is a later phase.
- Canonical source staging seam: dependencies resolve through a derived `.mars/staging/` tree with `lift_frontmatter` hook before discovery/hash/apply.
- Per-dialect frontmatter lift tables (Claude/Codex/Cursor/OpenCode) in `staging/lift.rs`; default/inferred Claude lift is idempotent on mars-native packages.
- `when_to_use` on `SkillProfile` with lowering to Claude/Pi and warn-drop on other harnesses.
- `dialect` key on `[dependencies.<dep>]` plus `.opencode`/`.cursor` discovery roots for inbound dialect inference.
- `[skills.<name>]` overlay carriage in config; applied at staging after lift (description, invocability, tool policy).
- Skills `disallowed-tools` canonical denylist.
- Shared `compiler/tool_policy.rs` for agent and skill tool gating (`tools:` list-or-map, `disallowed-tools:`, inline `mcp(...)` grants).

### Changed
- **Breaking:** Source discovery is a single bounded convention walk that finds `agents/` / `skills/` / `bootstrap/` directories (and a root `SKILL.md` fallback) at any depth, grounded to the shallowest package layer (deeper nested containers are ignored). Hidden dot-directories are skipped, so harness output surfaces (`.claude/`, `.codex/`, `.cursor/`, `.opencode/`) are no longer auto-scanned as discovery *sources* — import a foreign hidden layout explicitly via `subpath` + `dialect`. Replaces the previous fixed-top-level scan + container-root heuristic.
- **Breaking:** Removed legacy `mcp-tools:` / `mcp_tools` frontmatter field and `[agents|skills].tools.mcp` overlay key. Author whole-server MCP grants as `mcp(server)` entries in `tools:` (or `tools.allowed` overlays); per-tool grants use `mcp(server/tool)`.
- MCP emission (Phase 4b): `EffectiveToolPolicy` carries structured `mcp_allowed` / `mcp_disallowed` [`McpRef`] values; harness lowering and launch bundles project them via `project_mcp_ref` instead of verbatim `mcp-tools:` strings. Claude agents emit `mcp__…` tokens in `tools:` / `disallowed-tools:` (no `mcp-tools:` field); Claude skills grant MCP into `allowed-tools:` with a lossiness note. Unsupported projections (e.g. Claude `mcp(*/tool)`, Codex/Pi MCP) record lossiness and omit the token.
- Inbound lift: Claude `mcpServers` now appends `mcp(server)` entries to `tools:` instead of lifting to `mcp-tools:`.
- MCP input unification (Phase 2): inline `mcp(...)` entries in `tools:` / `disallowed-tools:` converge on one internal policy model; agent and skill parsers share the same key set.
- Skills use the same canonical tool schema as agents: `tools:` (list or allow/deny map), `disallowed-tools:`, and inline `mcp(...)` MCP grants. Shared parser in `compiler/tool_policy.rs`. Foreign `allowed-tools` lifts to `tools:` at staging.
- Agents honor `user-invocable` (was skills-only).
- Agent `model-invocable` / `user-invocable` a target cannot express now warn (deduped per item×target) instead of silent drop; same for skill tool fields a target cannot carry.
- Lossiness warnings surface only on `mars sync`, `mars upgrade`, `mars init`, and `mars check`; suppressed on validate, export, add, and other sync-pipeline commands.
- `[agents.<name>]` overrides now adjust description, invocation axes, and tool policy (was routing fields only).
- Codex skills: `model-invocable` warn-dropped (Codex reads invocation from sibling policy file, not SKILL.md) instead of emitting ignored `allow_implicit_invocation` frontmatter.
- Authoring foreign flat `allowed-tools`/`disallowed_tools` in a canonical skill now warns (use `tools:`) and the key is stripped from the `.mars/` store instead of leaking into canonical and native output.

## [0.9.0] - 2026-06-22

### Fixed
- Tool policy names now normalize from lowercase, aliases, and PascalCase into Mars snake_case before projecting to each harness native convention.
- `harness-overrides.<harness>` now skips invalid nested values without dropping valid sibling passthrough keys.

## [0.8.12] - 2026-06-21

### Fixed
- `mars agents list` / `mars skills list` now accept their filter flags (`--mode`, `--source`, `--type`, `--model-invocable`). The filters are global, so they work on both the bare form (`mars agents --mode subagent`) and the discoverable subcommand form (`mars agents list --mode subagent`), which previously errored with "unexpected argument".

## [0.8.11] - 2026-06-21

## [0.8.10] - 2026-06-20

### Added
- `mars models prompting <ref>` resolves agent refs before model aliases and shows model prompting guidance, with JSON output for the resolved ref.
- `mars models prompting` accepts `--refresh-models` / `--no-refresh-models` for launch-policy model resolution.

### Changed
- `harness-overrides.<harness>` is now exact target-native passthrough: Mars preserves the matching block in launch-bundle `execution_policy.native_config` without using it to replace top-level Mars fields.

### Fixed
- Mars tool fields now normalize separator-aware aliases like `ask_user` to PascalCase (`AskUser`) before lowering, while preserving unknown tool spellings exactly for the target harness.
- `mars models prompting <agent>` now reports guidance for the launch-resolved runnable model, including overlays, model policies, fallbacks, and model clearing.

## [0.8.9] - 2026-06-19

### Added
- `mars agents list` and `mars skills list` subcommands (aliases for the bare listing commands).

## [0.8.8] - 2026-06-17

### Changed
- Cursor promoted from experimental to first-class launch-bundle target. Removed `is_experimental` flag, `harness_stability` provenance, and user-facing warning. Removed `experimental` field from `HarnessDescriptor`.
- Inventory prompt-file idiom updated from `/tmp/<name>.md` to `$(meridian work path prompts/<name>.md)`.

### Fixed
- Test harness now sanitizes `MERIDIAN_MANAGED` env var, preventing `managed_cmd` output drift when tests run inside a meridian session.

## [0.8.7] - 2026-06-12

### Changed
- Claude target writes hook bindings to `settings.local.json` (gitignored) instead of `settings.json`. Hook commands embed machine-local cache paths that churn on every sync and every machine, so they no longer belong in the committed file. Existing managed hooks in `settings.json` are migrated out on next sync; user-owned hooks are preserved.

## [0.8.6] - 2026-06-12

## [0.8.5] - 2026-06-12

### Changed
- Moved fanout agent list from `[settings.meridian.agent_copy].fanout_agents` to `[settings.meridian.fanout].agents`. Old key parses with a migration warning and is ignored.
- Fanout agents that are native-for-harness now appear in both the Meridian spawn inventory and the native harness agents section.
- Fanout agent list matching is case-insensitive for native emission, consistent with inventory dual-listing.

### Fixed
- `mars build launch-bundle` now surfaces the deprecated `agent_copy.fanout_agents` migration warning in bundle `warnings`.

## [0.8.4] - 2026-06-11

### Added
- `fanout_agents` field in `[settings.meridian.agent_copy]` — selectively enable fanout qualification for listed agents when `include_fanout = false` globally.

## [0.8.3] - 2026-06-07

### Fixed
- Native agent compilation now uses the shared model routing evaluator for native model pinning, resolving auto aliases from `.mars/models-cache.json`, respecting model-policy fallback order, and clearing/skipping target-native model fields when no candidate routes to that harness.

## [0.8.2] - 2026-06-07

### Fixed
- `mars sync` lock rebuild now carries forward existing non-canonical target ownership records while refreshing canonical `.mars` records, instead of relying on a post-build safety-net pass.
- Local package discovery scans only `.mars-src/`, preventing package source `agents/` / `skills/` directories at the project root from being misread as local unmanaged items; `--force` also repairs stale canonical cache files left by the old discovery behavior.

## [0.8.1] - 2026-06-06

### Removed
- `launch_actions` experiment and `mars build launch-bundle --context` / `--transport` flags; launch-bundle schema reverted from v4 to v3 (revival tracked in #101).

### Fixed
- `.mars/native-agents.json` manifest determinism (sorted agent keys via `BTreeMap`) and write ordering (lock persisted before manifest projection).

## [0.8.0] - 2026-06-06
### Changed
- `[settings.agent_copy]` renamed to `[settings.meridian.agent_copy]` (clean break, no shim) — marks selective native copy as meridian-managed-only.
- `mars build launch-bundle --context` / `launch_actions` projection marked EXPERIMENTAL — not consumed by meridian; may be removed.

### Added
- `mars sync` summary now reports native-agent emissions and removals (`emitted N native agents` / `removed N native agents`, with per-line `+/-  <target>/<path> (native agent)`). Previously a managed/SuppressAll sync that pruned native agents printed "already up to date" — the prune was silent. Emissions are diffed against the prior lock so steady-state re-syncs stay quiet; counts also added to `--json`.
- `mars build launch-bundle --context` emits v4 `launch_actions` with subprocess/streaming `kind`, `cwd`, stdin, files, env, and bootstrap/turn protocol shapes for Cursor, Claude, Codex, OpenCode, and Pi.
- `.mars/native-agents.json` manifest after sync/link; launch-bundle inventory splits Meridian spawn agents from harness-native agents.

## [0.7.17] - 2026-06-06

### Fixed
- Harness exhaustion no longer falls through to unlinked `pi`; launch-bundle walks profile model-policy fallbacks instead.

## [0.7.16] - 2026-06-05

### Fixed
- `apply_apply_outcomes_to_lock()` dropped `.claude`/`.cursor`/`.codex` output records on `Updated`/`Installed` actions — `copy_decision()` lost ownership, triggering "not tracked by Mars" warnings every sync.
- `finalize()` dropped non-canonical ownership when native emission failed (I/O, permissions) — carry-forward now preserves old records, respecting explicit removals from reconciliation.

## [0.7.15] - 2026-06-04

### Changed
- Cursor compilation: `sandbox` and `approval` classified as `Approximate` (was `Dropped`). Cursor supports `--sandbox enabled/disabled` and `--force`/`--yolo` CLI flags; runtime projection is in meridian-cli.

### Fixed
- Compilation matrix docs: Cursor sandbox/approval tables and lossiness matrix updated to match code.

## [0.7.14] - 2026-06-02

### Fixed
- Git upgrade hints adapt canonical `github.com/org/repo` identities back to fetchable remotes before `git ls-remote`.

## [0.7.13] - 2026-06-02

### Changed
- `mars sync` upgrade hint now checks locked transitive dependencies, so stale compatible packages pulled through dependency ranges surface before `mars upgrade`.

## [0.7.12] - 2026-05-31

### Added
- `[settings.agent_copy]` — under `MERIDIAN_MANAGED=1` or `agent_emission = "never"`, selectively emit qualifying agents to native harness folders (e.g. `.claude/agents/`) by model→harness binding; `include_fanout` checks profile `model-policies`. Overrides blanket suppression; `agent_emission = "always"` still emits all.

### Changed
- One default harness order (`claude`, `codex`, `pi`, `cursor`, `opencode`) drives provider candidate order and empty-model routing.
- Native agent lifecycle out of `compiler/mod.rs`; one canonical scan; sync + link share post-target lifecycle.
- Native agent compile emits pinned model IDs for model aliases; raw and unpinned aliases still pass through.
- Lock-replay sync skips latest tag lookup unless upgrading or emitting the post-sync upgrade hint.
- `AGENTS.md`: clarify generated `mars.lock` is ignored local state and document `MERIDIAN_TASK_DIR` vs inherited `MERIDIAN_PROJECT_DIR` for nested Meridian commands.

### Fixed
- First-sync native ownership seeds from apply outcomes.
- `mars link` SuppressAll native reconcile scoped to linked harness only.

## [0.7.11] - 2026-05-30

## [0.7.10] - 2026-05-30

## [0.7.9] - 2026-05-30

## [0.7.9-rc.1] - 2026-05-30

## [0.7.8] - 2026-05-29

## [0.7.7] - 2026-05-29

### Fixed
- `mars list` / `mars list --status` / `mars why` / `mars doctor` showed every item N× (once per configured target). Used `canonical_flat_items()` for catalog views, `flat_items_for_target()` for divergence checks.
- Overlay model override (`mars.local.toml`) with incompatible provider now pivots to a compatible harness instead of hard-failing. E.g. profile `harness: codex` + overlay `model: sonnet` → routes to `claude` harness.
- Harness constraint error messages now include actionable fix suggestions.

## [0.7.6] - 2026-05-28

### Changed
- `AutoResolve` model aliases no longer require `provider` when `match` is specified. When provider is omitted, resolution searches across all providers in the models cache.

### Fixed
- Cursor routing: when probe slugs don't contain the model but `provider_constraint` says `cursor`, return `Constrained` evidence instead of hard-rejecting with `no_model_match`. Stale probe cache no longer routes cursor models to Pi.

## [0.7.5] - 2026-05-28

### Fixed
- Launch-bundle fixed-harness routing now soft-fails `no_model_match` only when the harness comes from a higher-precedence source than the selected model (for example CLI harness overriding profile model), clears the model to passthrough routing, and emits a warning instead of failing hard.

## [0.7.4] - 2026-05-27

### Added
- `effort: none` accepted as valid sentinel in agent frontmatter and model-policy overrides — means "no effort level" (same as omitting the field). Previously errored with invalid value.

### Fixed
- Cursor effort resolution now falls back to bare slug for models that have no effort-suffixed variants at all in the probe list (e.g. bare `composer`), instead of erroring with `NoEffortMatch`.

## [0.7.3] - 2026-05-26

### Changed
- Builtin model aliases now act as an empty-project fallback only; any dependency, project, or local model alias suppresses them so package-provided alias sets stay noise-free.
- Added a shared layered config boundary for project + project-local overlays (`user < project < project-local < command`) and switched launch-bundle policy resolution plus `mars models` routing to consume the same effective settings view.
- `mars.local.toml [settings]` now overlays the full settings surface using typed merge semantics (scalar replace, map key replace, array replace) instead of a routing-only subset.
- Config layering helpers moved into `src/config/layering.rs`, with explicit replace-by-key overlay semantics for keyed `[models]` and `[agents]` blocks.
- Runtime config consumers (`build` policy resolution, `mars models`, and `mars doctor`) now reuse a single effective project-config load path per command and share model-alias merge helpers instead of reloading/hand-merging config at multiple call sites.
- Added command-scoped lazy harness capability sessions for Pi/OpenCode/Cursor probe checks, memoized per harness and consumed by `mars models resolve` and launch-bundle routing so candidate evaluation only probes harnesses that are actually assessed.
- `mars models list` is now static by default (alias/catalog metadata only). Use `mars models list --live` for routed harness + availability details.

### Fixed
- `mars models resolve` passthrough success output no longer emits noisy catalog warnings when routing evidence is `confirmed` or `constrained`.
- `mars models list|resolve`, build policy routing, sync cache refresh, and validate compatibility checks now read settings from the merged effective project config (including `mars.local.toml`), and models commands return local-config parse/validation errors instead of silently defaulting.
- `mars sync` and native agent generation now honor `mars.local.toml [models]` overlays, and dependency alias conflict diagnostics are suppressed when a local model alias owns that name.
- `mars sync` now persists dependency alias winners in committed `mars.lock` (`dependency_model_aliases`) and no longer uses `.mars/models-dependencies.json` as alias authority.
- Raw model-id resolution and launch-bundle routing continue to honor `mars.local.toml` `harness_order` overlays while using lazy probe lookups, so local routing precedence remains consistent without eagerly probing unrelated harnesses.
- Cursor effort routing now reports typed failure causes (probe unavailable/empty, missing model prefix, missing effort variant), supports Composer bare-slug fallback when no effort variant exists, and emits precise launch-bundle errors instead of warning-string heuristics.

## [0.7.2] - 2026-05-24

### Fixed
- `resolve_harness_model` no longer prepends `{provider}/{model_id}` from alias `provider` before harness logic; native Codex/Claude receive bare model ids, Pi/OpenCode use probe slugs (fixes `gptmini` → `openai/…` on Codex and Pi).

## [0.7.1] - 2026-05-24

## [0.7.0] - 2026-05-23

### Fixed
- Canonical sync diff uses `.mars` lock outputs, avoiding false local-modified warnings when linked targets store different compiled checksums for the same item path.

## [0.6.6-rc.1] - 2026-05-23

### Changed
- Cursor native agent lowering now emits Cursor-specific markdown: one-line normalized `description`, `skills` passthrough, and policy-field lossiness preserved.
- Removed user-facing `[models.<alias>.native]` model override config.
- Cursor native agent `model` now uses the shared Cursor effort resolver against cached probe slugs (with conservative fallback to original token when no probe/candidate match applies).

## [0.6.5] - 2026-05-23

### Changed
- Launch-bundle auto-routing defaults to `harness_order = ["claude", "pi", "codex", "opencode", "cursor"]` when `settings.harness_order` is unset (explicit empty/invalid still falls through).
- Native harness routing (`claude`, `codex`) compares the requested model id against the cached models.dev catalog (same slug matching as Pi/OpenCode), not only alias/provider affinity.
- `mars models list` and `mars models resolve` now use the same routing evidence assembly as launch-bundle, including cached catalog slugs for native harness matching.
- Cursor probe prefix matching now requires an exact match or hyphen boundary, avoiding ambiguous prefix matches like `gpt-5` matching `gpt-55-*`.
- Bare agent model tokens infer `provider_for_order` from model id prefixes (e.g. `claude-opus-4-6` → `anthropic`).
- Linked-harness fallback walks `harness_order` and skips harnesses already rejected (`pi_incompatible`, `no_model_match`, …) instead of always selecting the first linked target.
- Auto-routing defers `Passthrough` harness selection until later candidates are evaluated, so native catalog matches (e.g. Codex for OpenAI models) win over earlier universal harness passthrough.
- Launch-bundle resolves Cursor `model + effort` into the exact probe slug as `harness_model` and clears `execution_policy.effort` when applied; Claude thinking variants are preferred when multiple slugs match.
- Cursor effort `medium`, `none`, `auto`, and `default` resolve to the unsuffixed base slug when the probe lists it (Cursor’s default tier), instead of requiring a `-medium` suffix.
- `routing.candidate_slugs` is diagnostic-only; consumers should run `harness_model` verbatim.
- `--refresh-models` on `mars models list|resolve`, `mars sync`, and `mars build launch-bundle`: force models.dev catalog refresh and run harness probes synchronously (no background `__refresh-probe` spawn on stale cache).
- `--no-refresh-models` on `mars build launch-bundle` matches models/sync: disk-only catalog and probe `Skip` (stale probe cache still used when present).
- `build launch-bundle` calls `ensure_fresh` for the models.dev catalog (default `Auto`: HTTP only when TTL stale) instead of read-only `load_models_cache`.

## [0.6.4] - 2026-05-23

### Added
- Cursor probe: `cursor agent --list-models` probe backed routing. Cursor changes from `UniversalPassthrough` to `ProbeBacked`; cursor models show `availability: runnable` when probe succeeds.
- Cursor probe cache: TTL cache (`cursor-probe.json`) with stale/miss/hit/unavailable outcomes and background refresh via `mars models __refresh-probe --target cursor`.
- Cursor prefix routing: `candidate_slugs` in launch bundle routing section carries all catalog slugs matching the requested model prefix; supports Python-side effort resolution.
- `classify_cursor` in availability layer: `CursorProbe`, `CursorProbeNegative`, `CursorProbeUnknown` sources in `AvailabilitySource`.
- Graceful degradation: no probe result, probe failure, or empty catalog all fall through to passthrough behavior (same as previous `UniversalPassthrough`).

### Changed
- Cursor harness changes from `HarnessClass::UniversalPassthrough` to `HarnessClass::ProbeBacked` in registry.

## [0.6.3] - 2026-05-23

### Fixed
- Pi probe reads CLI output from stderr when stdout is empty (Pi 0.75+), fixing false `pi_incompatible` and empty `model_slugs` in routing and `mars models list`.
- Pi `--list-models` space-separated tables with extra columns (context, max-out, …) now parse provider/model from the first two columns.

## [0.6.2] - 2026-05-22

## [0.6.1] - 2026-05-22

### Changed
- Launch-bundle model now optional; unset model routes to installed/default harness and leaves harness model empty for harness defaults.
- Default to RC release when no release label present on merged PR (previously skipped release entirely).

### Fixed
- Linked-target sync no longer deletes or overwrites hand-written files when the lock only tracks the same path under `.mars`. Orphan cleanup, Removed handling, and copy paths now require a per-target `OutputRecord`. Pre-existing untracked collisions are preserved with `target-unmanaged-collision`; `mars sync --force` and `mars link --force` adopt them and record ownership (`target-unmanaged-adopted`). Closes #60.
