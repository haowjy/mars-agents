# mars-schema

Package data reading, validation, translation and editing without host I/O; the binary
owns filesystem, network, orchestration, consumer config and target lowering. Keep the
crate usable on `wasm32-unknown-unknown`: the CLI and JavaScript callers share its types
and rules, so never add a binary-side copy.

## Reading and preparation

- Route authored definitions through the shared reader. Schema-v1 authored fields are
  strict; foreign and pre-v1 definitions go through translation and `set_aside_rejected_keys`
  so an uneditable dependency can still install. Preserve valid entries in `LoweredField`,
  `skill::LOWERED_KEYS`, and network host lists; losing a deny entry can widen permissions.
  Record lost deny/ask tools, disallowed tools, deny hosts and restriction fields as
  `field.restriction-lost` changes, exposed by `Prepared::lost_restrictions`.
- Ordinary reading leaves schema-v1 files as written; migration may repair removed
  fields because it writes the result back. Upgrade definitions without changing
  unrelated bytes when possible. Do not change a dependency's `requires-mars` during
  preparation: it gates whether the dependency can be read. Migration adds a missing
  floor when it can edit the manifest minimally; it leaves an existing requirement
  or an inline package table unchanged and provides advice when needed.
- A `mars.toml` without `[package]` is consumer configuration, not a package manifest.
  `project_authored_root` selects `.mars-src` with `AuthoredLocal` for consumer
  configuration, otherwise the project root with `Package`.
- Prepare before applying consumer filters and renames. Check skill body links against
  the actual installed layout after those choices: preparation cannot know whether a
  target from another package will be installed. Use `selection::select` and its
  `SelectionResult` for Agents, skills and bootstrap documents so callers agree.
- Read consumer dependency entries strictly. A package dependency's conflicting
  filters or `plugin` plus `dialect` warn instead, so a dependent can still sync.
- Preserve `.git` and `node_modules` in Mars skill payloads even though foreign
  skill imports exclude them: installed Mars packages depend on that behavior.
- Report wrong-transport MCP server fields as ignored rather than failing the parse,
  so a usable server can still install.

## Compatibility

- Dependency `version` and `requires-mars` / `requires-meridian` use different
  grammars: bare `0.20` is a caret range for a dependency and a minimum for a
  requirement. Keep their parsers distinct.
- `version::keeps_lock` keeps a lock under `PreferLockThenLatest` when every
  semver constraint allows it, it is not excluded, and no request asks for latest,
  without checking whether its tag still exists upstream. This lets a locked
  install replay without listing tags. Every constraint must hold at the chosen ref;
  incompatible pins and a pin beside a range fail.
- Preserve the published read, edit, preparation, selection and hook output shapes,
  including authored spellings and ordered values, across Rust and WASM. The fixtures
  under `fixtures/` exercise both implementations; review changes to expected output
  as contract changes. Keep `serde_json/preserve_order` off: feature unification
  would change the binary's JSON output. Use `OrderedValue` for authored order.
- Keep frontmatter size and nesting limits: YAML scanning grows quadratically with
  nesting, so a size limit alone does not bound parse time. Keep semantic parsing and
  structural scanning distinct when changing YAML handling.

## Config keys

`keys::KEYS` is the one table of config keys: which surfaces a key appears on, the Mars
that first read it (`since`), whether a value restricts, and whether it is current, an
alias, deprecated, removed (with advice, and a rewrite `mars migrate` applies: drop,
rename or a value) or `Elsewhere` (a key that belongs in a project's files, which the
user config warns about and drops). Add a new key as a row and nowhere else: no
removed-key list, version-floor constant or per-key check beside the table. A reader asks
`keys::removed_key`, `keys::removals`, `keys::dropped_keys`, `keys::elsewhere_key` or
`keys::since`.

A removed key is an error on every surface the row lists, a key no row names is a warning
(`diagnostic::unknown_key_message`, one text for packages and consumer config), and an
unknown value is an error. Don't give one surface a different answer for the same key. The
consumer config reader is `src/config/read.rs`; see `src/config/AGENTS.md`.
