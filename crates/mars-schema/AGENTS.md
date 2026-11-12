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

`keys.rs` is the table of gated keys (`GATED`): a key an older Mars ignores or misreads, with
the release that first read it. It lists keys Mars reads today and when they arrived, nothing
about keys it no longer reads. A new gated key is a row and nowhere else; say on the row's
comment what the older Mars does with the key.

A file's version floor is `keys::floor` over the keys it uses, and every caller gets those from
`keys::used(document, Document)`, which walks a parsed `mars.toml`. The uses that live in other
files are the caller's to add as `Used::hook` and `Used::profile_rule` (`check::package_floor`
does, and `mars check`, `mars version` and `mars migrate` all call it); the consumer reader
(`config/read.rs`) calls `used` with `Document::Consumer`. A gated row is `Gate::Present` (the
key's presence) or `Gate::Rules` (each rule whose action restricts: `deny` and `ask`, not
`allow`). A consumer floor is reported only above `MIN_MARS_VERSION_SINCE`, since a Mars older
than that ignores `min_mars_version`.

What a reader does with a key it has no field for depends on the table, nothing else:

- A table of entries (`[settings.tools]`, a profile's `tools`, `[dependencies]`): the key is an
  entry, and a key or value that is not a valid one is an error at its line.
- A table of fields (`[settings]`, `[agents.<name>]`, `[package]`, frontmatter): the key is not
  read and stays in the file. It is one `Info`, `field.unknown`, built by
  `diagnostic::unknown_field`, with the text `unknown_key_message` builds. Don't add a code, a
  level or a text for a particular key.
- A table that refuses what it does not define (a model-policy `override`, a project's
  `hook.toml`): an error that lists the keys it takes.

A value of a key Mars reads stays closed: an unknown value is an error listing what is allowed.

The only code that names an old key is a translator from another format. `upgrade::PRE_V1` is
the pre-v1 Mars grammar: every key and value the pre-v1 upgrade drops, renames, moves or
rewrites. The upgrade functions iterate it, `mars migrate` runs the same conversion on a schema 1 file
(`migrate_definition`, limited to `PRE_V1`'s keys), and the strict readers ask it
(`PreV1::key`, `PreV1::value`), so the file the reader refuses with `field.removed` and one
text is the file `mars migrate` converts. The spellings of the Agent deny list come from
`tool_policy::DISALLOWED_SPELLINGS`, which the translators read too. A row is a conversion; a
key the upgrade only sets aside (`permission` as a map) is dropped with `field.restriction-lost`.
`upgrade::tests` checks this against the list. It is an error and not an unread key because a schema 1 file in
this format would lose a restriction silently (`disallowed-tools`, `network_deny`, a `permission`
map); `read::installed` and `set_aside` key on that error, so a translated definition still
records `field.restriction-lost`. A new key goes in `PRE_V1` only as a conversion the upgrade performs, and `upgrade::tests` fails when a row is refused but not converted, or converted but not refused.
