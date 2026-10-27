# mars-schema

Pure, host-independent schema logic for Mars package files. This crate owns authored
vocabulary, parsing, validation, normalization, and lossless editing. The `mars-agents`
package owns filesystem, network and process I/O, consumer configuration, orchestration,
host capability discovery, and target lowering.

Do not add filesystem access, network clients, process execution, or host-dependent
behavior. Keep `cargo check -p mars-schema --target wasm32-unknown-unknown` green.
Values shared with the runtime are defined here and consumed directly; do not add
duplicate binary-side types or re-export shims. Each rule has one owner below; the binary
consumes the result and never re-checks it (a second copy of a rule in the binary is a bug).

## Who owns what

| Module | Owns |
|---|---|
| `read` | `read(path, text)` and `read_definition(kind, text)`: the one way an authored definition is read; `installed_agent`/`installed_skill`, the upgrade-then-read every reader of an installed `.mars/` tree uses when the strict reading reports a pre-v1 spelling; `Reading`, `DefinitionRead`, `Valid` |
| `agent`, `skill`, `manifest` | The field readers for Agents, skills and the `[package]` table; the removed-field and removed-value tables (`RemovedField`, `RemovedValue`, in `diagnostic`); `agent::LoweredField` and `skill::LOWERED_KEYS`, the Agent and skill keys Mars lowers itself, which set-aside and native lowering both read |
| `harness_overrides` | `harness-overrides` parsing and `HarnessOverrides`, shared by the Agent and skill readers; `mask_consumer_namespaces`, the authored text with each consumer-namespace entry blanked by byte span, for a caller that scans what a package ships |
| `prepare` | `prepare`: classify one root, select, upgrade or translate, read; `Prepared`, `PrepareContext`, `Classification` |
| `dispatch` | Classification order, layout choice, consumer-configuration detection, the dialect prediction (`DialectChoice` in `dialect`) |
| `discover` | The one bounded walk (`walk`, `walk_dirs`, `excluded`), the Mars convention scan, payload assignment |
| `claude/` | The Claude layout: `manifest` (plugin and marketplace manifests, merge and `strict` rules), `discover` (items and where they land), `translate` (skills and Agents tables), `checks` (warnings that depend on where a skill landed) |
| `lift` | The Codex, Cursor and OpenCode lifts; the shared tool-name rewrite |
| `set_aside` | The class guard: after a foreign or pre-v1 definition is translated, every key the strict reader still rejects is moved under `harness-overrides.<dialect>` or dropped, with one warning. A key Mars lowers itself (`agent::LoweredField`, `skill::LOWERED_KEYS`: the lists native lowering reads too) is never moved: a mapping or list loses only its rejected entries and a scalar is dropped; `network-allow` and `network-deny` are treated the same, though Mars does not lower them, so one bad host never takes the valid ones with it. A rejected `permission` map is OpenCode's, so its note says to declare `dialect = "open-code"`, not Meridian Flow's advice. A lost restriction is a change with its own code, `field.restriction-lost` (`Prepared::lost_restrictions` filters them, so a sync prints them every run and `mars migrate` lists them with the rest); `harness_overrides::move_keys_under` is the one mover |
| `inspect`, `normalize` | `dispatch` and `prepare` over a whole repository; the generated `mars.toml` and provenance |
| `upgrade` | The pre-v1 to v1 conversion, shared by `prepare` and `mars migrate` (definitions; a manifest's `schema` and removed `visibility`; it never touches `requires-mars`), and `ensure_requires_mars_floor`, which adds a missing floor and reports an existing requirement it will not rewrite |
| `package` | Versioned package metadata, `classify_manifest`, `parse_requirement` and `requirement_has_floor`: the `requires-mars` floor rule, and the advice `mars check` and `mars migrate` share |
| `item` | `ItemKind` and `DefinitionKind` (Agent, Skill) |
| `dialect`, `harness` | `Dialect`, `DialectChoice`, `HarnessId`, `harness::names()`; the only harness-name parser |
| `tool_names`, `tool_natives`, `tool_policy`, `mcp`, `model_policy`, `invocability` | The vocabularies and parsers for `tools`, MCP references, model policies and the invocability axes |
| `network`, `permission_set` | `NetworkPolicy` and `NetworkHost` (the host grammar), which the Agent reader, a model-policy override and the binary's configuration layers all parse; `PermissionSet`, one layer of an additive set (a list adds, `{replace: [...]}` clears the lower layers), which `network-allow` and `network-deny` use here and the binary's tool and host layering use there |
| `frontmatter`, `ordered`, `edit`, `markdown` | The structure-preserving frontmatter parse, `OrderedValue`, `emit_value` and `rewrite_block`, fenced-block recognition |
| `tree` | `TreeSource`; `FileTree` (WASM and fixtures) and `SubTree` |
| `diagnostic`, `plugin_metadata` | `SchemaDiagnostic` and its codes; the generated root-file table, `PluginSentinel::validate` with the `SentinelError` naming the entry that failed, and the generated-plugin marker check |

## Preparation and classification

`prepare` is the one-root reading core. `dispatch` classifies the root in the order of
`mars-normalize` (generated output, then a Mars manifest, then a declared dialect, then the
dialect predicted from evidence in the root, then empty) and chooses a layout. `Classification`
is the one classification and `DialectChoice` the one record of a dialect and its evidence;
`inspect` reports a `Kind` derived from the classification and layout, not a parallel enum.

A root with no manifest is a pre-v1 Mars package, read through the upgrade, only when it is declared
`mars-native`; every other one is read under the dialect declared or predicted. What only pre-v1
Mars wrote is never evidence for the root. `upgrade::lift_mars_only_keys` applies it per key,
before the rules of whichever dialect reads the definition: the invocability renames, the
discarded Agent fields (`RemovedField::discarded`), the removed values (`REMOVED_AGENT_VALUES`),
`move_agent_fields` (a `permission` of `read` or `edit` to `harness-overrides.meridian-flow`,
`resident-rearm-budget` and `meridian-capabilities` to `harness-overrides.meridian-cli`, one conflict
rule for all three: a block already set wins, and its top-level twin is dropped with a note, a
lost restriction when the twin is `permission`) and the removed model-policy override keys.
`permission` is not on that list as a key, because OpenCode defines it too. Meridian Flow defines one key,
`permission`, and the Meridian CLI `resident-rearm-budget` and `capabilities`, so those moves are
the only writers of the two reserved consumer namespaces; the legacy upgrade drops what it cannot
convert.

`mars migrate` prepares through `prepare_for_migration`, which runs
`upgrade::relocate_removed_fields` — `rename_consumer_namespaces` (the short `flow` and
`meridian` namespaces to `meridian-flow` and `meridian-cli`, for agents and skills) and
`move_agent_fields` — on a schema 1 definition,
because it writes the result back. Every reading caller uses `prepare`, which reads an authored
schema 1 file exactly as written: a removed key there is a `field.removed` error, and only
the keys `mars migrate` can move are the ones it repairs.

Prediction from the items reads each dialect's own vocabulary and holds none itself:
`claude::translate::EVIDENCE_KEYS` (narrower than that module's moved-key lists, because `model`,
`effort`, `tools` and `hooks` also appear in Mars's own files, and the rewritten spellings are read in
code, so it is not derived from them) and `lift::is_opencode_agent` (a `permission` map or a `tools` map
of booleans: the two spellings `lift_opencode_agent` translates). An Agent decides what a skill
cannot: Claude evidence on an Agent wins, then OpenCode's, then Claude evidence that came only from
skills. `allowed-tools` is not Claude evidence on a skill, because the Agent Skills specification
defines it (`STANDARD_SKILL_KEYS`). A tree predicted OpenCode keeps the layout found for it, so
Claude's notices (commands, hooks, MCP servers) still print.

`PrepareContext` is `Package` (a package root), `AuthoredLocal` (a consumer project's
`.mars-src`) or `Project` (a directory that is either: a `mars.toml` with no `[package]` is
consumer configuration and the tree is read as authored content). The crate decides; callers do
not re-parse `mars.toml` to pick a context. `project_authored_root` gives a writer, `mars migrate`, the
directory to read and write (`.mars-src` or the root) with the context.

`prepare` returns source and output paths, normalized bytes, authored-file change records,
payload mappings, and source-keyed structured diagnostics. Source paths are normalized
package-relative identities and never describe materialization storage. Consumers apply filters,
renames, overlays, and other consumer configuration only after preparation. `prepare` lists
payload but never reads payload bytes. Diagnostic codes are the crate's own strings; the binary
passes them through.

## Discovery

One bounded directory walk (`discover::walk`) owns what no scan enters: hidden directories, the
skip list, depth five, `[plugin].dir` (read from `mars.toml`) and any directory holding a valid
generated-plugin marker, checked as the walk is about to enter a directory and never by scanning
the tree. The Mars convention scan over it serves Mars packages and the Codex, OpenCode and
Cursor lifts, with output paths equal to source paths. The Claude layout and the binary's
bootstrap scan (`discover::walk_dirs`) use the same walk.

The Claude layout serves `claude` and `agent-skills`. A manifest list adds to the default
skills folder and replaces the default Agents and commands folders. Items are the default
`skills/<name>/SKILL.md`, listed paths, or for a bare tree every `SKILL.md` directory within five
levels without entering a found skill; each is relocated to `skills/<slug>/` or
`agents/<slug>.md`. Payload is assigned before selection, so a nested skill is never its
parent's payload. A foreign repository's skill leaves out `.git` and `node_modules`; a Mars package's
skill carries everything it holds, as every Mars release has.

## inspect and normalize

`inspect(tree, hint)` reports the kind (`schema-1`, `mars-legacy`, `schema-newer`,
`marketplace`, `plugin`, `skill-tree`, `invalid`, `empty`), the dialect with its evidence and
whether it was declared or predicted, other harnesses' copies (`also_found`, never read), a
marketplace's plugins with their sources (local, remote coordinates, or unsupported) and, for
local ones, their items, and a plugin's or skill tree's items with payload listings.

`normalize(request)` runs `prepare` on each selected root (a marketplace plugin is its own
package) and adds a generated `mars.toml` (`schema = 1`, name from the plugin, the marketplace
entry, or the repository name plus the subpath) and provenance. A Mars package is the identity
case. A package-level error (two skills with one slug, conflicting manifests) leaves the package
holding nothing to write, with the error in its diagnostics.

A plugin whose files are in another repository has one protocol: the caller fetches the tree
and calls `inspect` or `normalize` on it with `Hint.plugin` set to the marketplace entry (the
resolver does this for a dependency rooted at a fetched plugin). A fetched root that is itself a
marketplace is refused (`marketplace.nested-marketplace`); normalizing the marketplace reports
each remote entry as `marketplace.remote-not-fetched`.

## Readers and the upgrade

`agent`, `skill`, and `manifest` are the only readers for schema-v1 definitions. An Agent or
skill is read one way, `read::read_definition`: the structure-preserving parse (an anchor or a
document-end marker is a diagnostic beside the fields, never a refusal to read them), then the
kind's field reader. `agent::read`, `skill::read`, `read::read` and `prepare` are that
function, and the binary reads definitions through them (`DefinitionRead::into_valid` for a
caller that refuses a file with any error). Callers consume these types directly rather than
reading owned frontmatter keys themselves.

Only an authored schema-v1 file is refused for its fields. A translated definition (foreign or pre-v1) is never
left holding a key or value the strict reader rejects: `set_aside` moves it aside with a warning, so a package
its consumer cannot edit still installs.

Unknown fields are kept in the authored file but reach no native file (`field.unknown` points
to `harness-overrides.<harness>`); removed fields are errors (`field.removed`). Each key
vocabulary is one table iterated by every consumer: `REMOVED_AGENT_FIELDS` and
`REMOVED_SKILL_FIELDS` serve the reader and, through their `discarded` entries, the upgrade
(the skill table's discarded keys are also the import's skill tool keys, `skill::tool_keys`);
`claude::translate::move_to_claude_overrides` is the one mover of Claude-only keys, for the upgrade and
the Claude translation: it moves them under `harness-overrides.claude` instead of deleting them. An
override already there wins and the key it displaces is a warning; a non-mapping `harness-overrides`
leaves the keys in place with a warning.

`upgrade` is the single pure pre-v1-to-v1 conversion used by both package preparation and
`mars migrate`. Definition upgrades edit only changed top-level blocks and keep all other
authored bytes. A non-block top-level mapping or failed semantic verification falls back to a
reported full frontmatter rewrite. Unchanged definitions and schema-v1 manifests remain
byte-identical. `requires-mars` is not part of the upgrade: the requirement gates whether a
package may be read at all, so `prepare` never changes a dependency's requirement. `mars migrate`
adds `>=0.20.0` to a manifest that has none, prints `mars check`'s advice for an existing
requirement that admits an older Mars (leaving its value and comment), and leaves a manifest it
cannot edit minimally (an inline `package` table) alone with that advice instead of failing.
What preparing changed in a file, whether a pre-v1 upgrade or a foreign translation
dropped, moved or repaired it, is one channel, `Prepared.changes`; `Prepared.diagnostics` holds only
what discovery and translation could not convert. `Prepared::changed_files` counts the authored files
whose bytes changed (not a generated `mars.toml`), and the binary reports that count, never the codes.

Frontmatter is bounded (`MAX_FRONTMATTER_BYTES`, 64 KiB: `frontmatter.too-large`; `MAX_FLOW_DEPTH`, 64
open brackets over the raw text: `frontmatter.too-deep`) in the one parse every reader shares,
because the YAML scanners are quadratic in nesting depth (64 KiB of `[` took 5.4 s). A refused block
reads as `fields: null` with that one diagnostic.

`yaml-rust2` (with default features disabled) supplies the event scan that distinguishes YAML
anchors, aliases, and explicit document-end markers from identical text inside scalars.
`serde_yaml` remains the semantic parser and validity authority; the event scan does not
produce field values.

## Published contract

The public read/edit boundary is `read::read`, `edit::emit_value` and `edit::rewrite_block`.
`read(path, text)` picks the reader from the package-relative path (`mars.toml`,
`agents/<slug>.md`, `skills/<slug>/SKILL.md`; any other path is an error, never a default) and
returns one `DefinitionRead`: ordered `frontmatter` pairs, typed `fields`, `body`,
`diagnostics`. Typed fields serialize with the spellings the file uses (kebab-case keys and
values, `xhigh`, `read-only`), internal flags are `#[serde(skip)]`, and wrappers are
`#[serde(transparent)]`; a refactor of `AgentProfile` that changes the serde shape changes the
published contract and `fixtures/read/`. `prepare` embeds the same shape and keeps only
package-level diagnostics (discovery, translation) in its own map; `Prepared::all_diagnostics`
merges both. `Diagnostic.range` is for fence and YAML-syntax errors only (divergence V3) and no
reader sets it yet.

Emission has one scalar rule for keys and string values: plain only for a safe word
(`[A-Za-z_][A-Za-z0-9_./-]*`, no YAML boolean, null, number or infinity word), otherwise JSON
double-quoted with every non-printable, line-separator and BOM character escaped. Multi-line
strings are quoted, never block scalars. `emit_value(value, indent)`: `indent` is the owning
key's column; a scalar is one line with no leading newline; a non-empty collection starts with
`\n` and its children sit at `indent + 2`; output never ends in a newline. Integers beyond
±(2^53 − 1), NaN, Infinity and keys written as over 1024 characters are refused (floats of any
finite magnitude are emitted so both parsers read the same value). `rewrite_block` returns the
whole block between the fences, keys in order, ending in exactly one newline. Raw values are
`ordered::OrderedValue` (mapping keys in authored order, serialized in order, strict about NaN
and big integers), never `serde_json::Value`, whose keys sort; `serde_json/preserve_order`
stays off because it would reorder every binary `--json` output. Native config for the bundle
is converted to `serde_json::Map` at the boundary.

## WASM and fixtures

`mars-schema-wasm` serializes with `Serializer::json_compatible()` (plain objects, `null`) and
exposes `read`, `emitValue`, `rewriteBlock`, `vocabulary`, `prepare`, `inspect` and `normalize`
as `@meridian-flow/mars-schema` (trees are `Map<path, Uint8Array>`). `prepare` is a supported
export on its own: the way to read one root without choosing items. The npm `node/`, `web/` and
`fixtures/` are built by `scripts/build-schema-wasm.sh`, never committed. The script builds the
`wasm` Cargo profile (opt-level `z` for every dependency, so about a quarter smaller than
`release` and about 60% slower to parse); the native `release` profile is untouched.

`fixtures/<suite>/<case>/` are the published contract: `read`, `emit`, `upgrade`, `prepare`,
`normalize` and `vocabulary`. Fixtures are checked in Rust (`tests/`) and in Node
(`crates/mars-schema-wasm/run-fixtures.mjs`, deep equality on every suite; emit cases are
spliced into a frontmatter and read back by `read` and the `yaml` library). Regenerate expected
output with `MARS_UPDATE_FIXTURES=1 cargo test -p mars-schema` and review the diff.
`crates/mars-schema-wasm/sweep-rewrite.mjs <dir>...` checks read → rewriteBlock → read on real
files.
