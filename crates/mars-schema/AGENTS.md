# mars-schema

Pure, host-independent schema logic for Mars package files. This crate owns authored
vocabulary, parsing, validation, normalization, and lossless editing. The `mars-agents`
package owns filesystem, network and process I/O, consumer configuration, orchestration,
host capability discovery, and target lowering.

Do not add filesystem access, network clients, process execution, or host-dependent
behavior. Keep `cargo check -p mars-schema --target wasm32-unknown-unknown` green.
Values shared with the runtime are defined here and consumed directly; do not add
duplicate binary-side types or re-export shims.

`package` owns versioned package metadata and manifest classification. `upgrade` is the
single pure pre-v1-to-v1 conversion used by both package preparation and `mars migrate`.
Definition upgrades edit only changed top-level blocks and keep all other authored bytes. A
non-block top-level mapping or failed semantic verification falls back to a reported full
frontmatter rewrite. Unchanged definitions and schema-v1 manifests remain byte-identical. The
fixtures under `fixtures/upgrade/` are the published upgrade contract.

`prepare` is the one-root reading core. `dispatch` classifies the root in the order of
`mars-normalize` (generated output, then a Mars manifest, then a declared dialect, then the
dialect predicted from evidence in the root, then empty) and chooses a layout. One bounded
directory walk (`discover::walk`) owns what no scan enters: hidden directories, the skip list,
depth five, `[plugin].dir` (read from `mars.toml`) and any directory holding a valid generated-plugin
marker (checked as the walk is about to enter a directory, never by scanning the tree). The Mars
convention scan over it (`discover`) serves Mars packages and the Codex, OpenCode and Cursor lifts,
with output paths equal to source paths; the Claude layout and the binary's bootstrap scan
(`discover::walk_dirs`) use the same walk. The Claude layout (`claude/`) serves `claude` and
`agent-skills`: `claude::manifest` parses `plugin.json` and `marketplace.json` and applies
Claude's merge and `strict` rules (a list adds to the default skills folder and replaces the
default Agents and commands folders), `claude::discover` finds the items (the default
`skills/<name>/SKILL.md`, listed paths, or for a bare tree every `SKILL.md` directory within five
levels without entering a found skill) and relocates each to `skills/<slug>/` or
`agents/<slug>.md`, and `claude::translate` and `claude::checks` hold the skills and Agents tables
and the warnings that depend on where a skill landed. Payload is assigned before selection, so a
nested skill is never its parent's payload. `prepare` returns source/output paths, normalized
bytes, authored-file change records, payload mappings, and source-keyed structured diagnostics.
Source paths are normalized package-relative identities and never describe materialization
storage. Consumers apply filters, renames, overlays, and other consumer configuration only after
preparation. `prepare` lists payload but never reads payload bytes; `FileTree` is its in-memory
WASM and fixture adapter and `SubTree` roots a tree below a directory. The cases under
`fixtures/prepare/` are the serialized preparation contract.

`inspect` and `normalize` are `dispatch` and `prepare` over a whole repository, not a second
pipeline. `inspect(tree, hint)` reports the kind (`schema-1`, `mars-legacy`, `schema-newer`,
`marketplace`, `plugin`, `skill-tree`, `invalid`, `empty`), the dialect with its evidence and
whether it was declared or predicted, other harnesses' copies (`also_found`, never read), a
marketplace's plugins with their sources (local, remote coordinates, or unsupported) and, for
local ones, their items, and a plugin's or skill tree's items with payload listings.
`normalize(request)` runs `prepare` on each selected root (a marketplace plugin is its own
package; a remote plugin's tree comes in `remote_trees`, fetched by the caller after it
inspected the tree with `Hint.plugin` set to the entry) and adds a generated `mars.toml`
(`schema = 1`, name from the plugin, the marketplace entry, or the repository name plus the
subpath) and provenance. A root normalized with `Hint.plugin` for a remote entry (the resolver
roots a dependency at the fetched plugin and prepares it through its entry) gets the same
nested-marketplace refusal as one that arrives in `remote_trees`. A Mars package is the identity case. A package-level error (two skills
with one slug, conflicting manifests) leaves the package holding nothing to write, with the
error in its diagnostics. `fixtures/normalize/<case>/` holds `input/`, `request.json`,
`inspection.json`, `normalized.json` and the file maps under `expected/<package id>/`.

`agent`, `skill`, and `manifest` are the only readers for schema-v1 definitions. An Agent or
skill is read one way, `read::read_definition`: the structure-preserving parse (an anchor or a
document-end marker is a diagnostic beside the fields, never a refusal to read them), then the
kind's field reader. `agent::read`, `skill::read`, `read::read` and `prepare` are that function,
and the binary reads definitions through them (`DefinitionRead::into_valid` for a caller that
refuses a file with any error). Their serialized reading and diagnostics are pinned by
`fixtures/read/`; callers consume these types directly rather than reading owned frontmatter
keys themselves. Unknown fields are
kept in the authored file but reach no native file (`field.unknown` points to
`harness-overrides.<harness>`), while removed fields are errors. The Claude translation moves
Claude-only keys under `harness-overrides.claude` instead of deleting them.

`yaml-rust2` (with default features disabled) supplies the event scan that distinguishes YAML anchors, aliases, and explicit document-end markers from identical text inside scalars. `serde_yaml` remains the semantic parser and validity authority; the event scan does not produce field values.

The public read/edit boundary is `read::read`, `edit::emit_value` and `edit::rewrite_block`.
`read(path, text)` picks the reader from the package-relative path (`mars.toml`,
`agents/<slug>.md`, `skills/<slug>/SKILL.md`; any other path is an error, never a default) and
returns one `DefinitionRead`: ordered `frontmatter` pairs, typed `fields`, `body`, `diagnostics`.
Typed fields serialize with the spellings the file uses (kebab-case keys and values, `xhigh`,
`read-only`), internal flags are `#[serde(skip)]`, and wrappers are `#[serde(transparent)]`; a
refactor of `AgentProfile` that changes the serde shape changes the published contract and
`fixtures/read/`. `prepare` embeds the same shape and keeps only package-level diagnostics
(discovery, upgrade) in its own map; `Prepared::all_diagnostics` merges both. `Diagnostic.range`
is for fence and YAML-syntax errors only (divergence V3) and no reader sets it yet.

Emission has one scalar rule for keys and string values: plain only for a safe word
(`[A-Za-z_][A-Za-z0-9_./-]*`, no YAML boolean, null, number or infinity word), otherwise JSON
double-quoted with every non-printable, line-separator and BOM character escaped. Multi-line
strings are quoted, never block scalars. `emit_value(value, indent)`: `indent` is the owning
key's column; a scalar is one line with no leading newline; a non-empty collection starts with
`\n` and its children sit at `indent + 2`; output never ends in a newline. Integers beyond
±(2^53 − 1), NaN, Infinity and keys written as over 1024 characters are refused (floats of any
finite magnitude are emitted so both parsers read the same value). `rewrite_block` returns the whole block between the fences, keys in order,
ending in exactly one newline. Raw values are `ordered::OrderedValue` (mapping keys in authored
order, serialized in order, strict about NaN and big integers), never `serde_json::Value`, whose
keys sort; `serde_json/preserve_order` stays off because it would reorder every binary `--json`
output. Native config for the bundle is converted to `serde_json::Map` at the boundary.

`mars-schema-wasm` serializes with `Serializer::json_compatible()` (plain objects, `null`) and
exposes `read`, `emitValue`, `rewriteBlock`, `vocabulary`, `prepare`, `inspect` and `normalize` as
`@meridian-flow/mars-schema` (trees are `Map<path, Uint8Array>`). The npm `node/`, `web/` and `fixtures/` are built by
`scripts/build-schema-wasm.sh`, never committed. Fixtures are checked in Rust (`tests/`) and in
Node (`crates/mars-schema-wasm/run-fixtures.mjs`, deep equality on every suite; emit cases are
spliced into a frontmatter and read back by `read` and the `yaml` library). Regenerate expected
output with `MARS_UPDATE_FIXTURES=1 cargo test -p mars-schema` and review the diff.
`crates/mars-schema-wasm/sweep-rewrite.mjs <dir>...` checks read → rewriteBlock → read on real
files.
