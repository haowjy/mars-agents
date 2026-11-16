# mars-schema

Package data reading, validation, translation and editing without host I/O; the binary
owns filesystem, network, orchestration, consumer config and target lowering. Keep the
crate usable on `wasm32-unknown-unknown`: the CLI and JavaScript callers share its types
and rules, so never add a binary-side copy.

## Reading and preparation

- Route authored definitions through the shared reader. Schema-v1 authored fields are
  strict; foreign definitions go through translation and `set_aside_rejected_keys`
  so an uneditable dependency can still install. Preserve valid entries in `LoweredField`,
  `skill::LOWERED_KEYS`, and network host lists; losing a deny entry can widen permissions.
  Record lost deny/ask tools, disallowed tools, deny hosts and restriction fields as
  `field.restriction-lost` changes, exposed by `Prepared::lost_restrictions`.
- An Agent with no frontmatter is `frontmatter.missing` only where the file is schema 1: `read` and `prepare` of a `schema = 1` package go through `read_authored`. Installed trees and foreign layouts use `read_definition`, because their body-only Agents were valid when written. A consumer's `.mars-src/` and a manifest-less `mars-native` tree are classified `Schema1`, so they are checked like a package.
- Reading leaves a schema-v1 file as written; preparation rewrites only foreign
  definitions. Do not change a dependency's `requires-mars` during preparation: it gates
  whether the dependency can be read.
- A `mars.toml` without `[package]` is consumer configuration, not a package manifest.
  `PrepareContext::Project` reads the project root as a `Package`, or, for consumer
  configuration, `.mars-src` as `AuthoredLocal`. A `[package]` with no `schema` is an error
  (`manifest.schema-missing`), not a package to upgrade.
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

## Resolution

`resolve::resolve` is one pure function over an `Answers` lookup: it decides versions and never fetches, so `mars sync` (answering each lookup as it is asked) and Flow (answering from a map) cannot choose differently. Keep it free of I/O and of any map whose iteration order reaches the result: `BTreeMap` or sorted, so the same answers give the same outcome, restarts included. A tree answer is a `TreeSource` read on demand, never a copy of the files, and `bounds.bytes` counts what is read through it. `Failure::message` is the one formatter of a failure's text; the CLI maps a failure to its own error type and keeps the text. `Answer::Failed(FailedFetch)` says why a lookup failed: a fetch's text, or a wrong `subpath`, which the core reports as `Failure::Subpath` itself, so no answerer rebuilds a failure from a side map. A version chosen before a restart (`Chosen`) and an engine exclusion belong to the `SourceId` they were made for; a kept choice is reused only while it meets every current constraint, and an edge back to a package still being walked is held to its constraint like any other. A new failure kind needs its message there and its mapping in the CLI's `answers.rs`.

`resolve::wire` is Flow's JSON surface (the `resolve` export): its `MapAnswers` answers a lookup from the input's list, and an answer with no match leaves the need open. Every kind of answer keeps one per key and the later one in the list wins; a tree need takes the answer for its own `(url, commit, path)`, else the nearest folder above it, else the whole repository's, so the result never depends on the order of `answers`. A new failure kind also needs a `FailureBody` variant there, its type in `npm/@meridian-flow/mars-schema/index.d.ts`, and a `fixtures/resolve/` case: `steps.json` holds a base input and the answers each call adds (file contents as text), `expected.json` the output of every call, rewritten by `MARS_UPDATE_FIXTURES=1`. Flow stores `resolved.lock` verbatim as mars.lock's dependency rows, so a field added to `LockedDependency` reaches its stored data.

## Tool projection for Meridian Flow

Flow has no per-server MCP tools; `tool_natives::FLOW_CONNECTION_TOOLS` is the one list of the connection tools that `mcp(...)` rules and MCP wire tokens lower onto, and a new connection tool joins by being listed there. Flow cannot scope them to one server, so `project_flow_connection_tools` errs toward fewer connections: a deny or ask of any `mcp(...)` lands on every one of them (the `note` says it covers every server, which can be broader than written), and an allow or ask under an allow-list (`*` that is not `allow`) gives none and stays `known` with a note, which the consumer shows as a warning (`unscopable` is refused, so it must not be used here). An allow with no allow-list changes nothing, since the tools are already available. Other hosts' MCP lowering must not change when Flow's does.

## Models catalog

`models.rs` is the models.dev row (`CachedModel`) and the pure alias resolver over rows
(`auto_resolve_all`, `glob_match`): the binary's snapshot and Flow's rows go through the same code.
Providers compare by `catalog_provider_canonical` of the raw models.dev key (`provider_key`), never
by display name: `meta` and `meta-llama` both display as "Meta". Fetching, the snapshot file and the
per-project provider filter are the binary's.

`effort.rs` is the one effort rule (`resolve_effort`, `harness_levels`): a request moves to the nearest level on the ladder (a tie goes up, set by `TIES_GO_UP`)
the model's row and the host both take, and the move is reported. Both the CLI and the WASM
`resolveEffort` call it; a host's accepted levels are recorded there, not in an adapter. A model's
reasoning is `ModelReasoning`: unknown, does not reason, or reasons with whether it can be switched off
and the levels its row names, if any. A model that reasons with no named levels takes the whole ladder,
less `none` when it cannot be switched off; the listing shows the shape, never that set.

## Checking

`check.rs` is what `mars check` and the WASM `check` report about a package's own files. A rule about an Agent, a skill or `mars.toml` goes there, never in `src/cli/check.rs`. It reports what is true of the package wherever it runs: what a harness target can't express, or an older Mars can't read (the `requires-mars` floor, hooks, lost-in-lowering fields), stays in the binary. Grade a finding by what the author loses: `Error` it won't load or install, `Warning` something written won't take effect, `Info` nothing written is lost. For each path the result is `prepare`'s findings plus the check's own, so dropping the check's codes must leave what `prepare` returns for the path; both runners hold that. A new code is a fixture under `fixtures/check/` and a row in the code lists of both runners.

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
does, and `mars check` and `mars version` both call it); the consumer reader
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
  level or a text for a particular key; the one exception is `retired.rs`, below.
- A table that refuses what it does not define (a model-policy `override`, a project's
  `hook.toml`): an error that lists the keys it takes.

A value of a key Mars reads stays closed: an unknown value is an error listing what is allowed.

`retired.rs` is the one table of keys Mars moved off: (scope, key, anchor on
`docs/config/retired-keys.md`). A reader that meets one emits `field.retired` as a Warning, because
the user loses what it set, often a restriction (`disallowed-tools`, `permission`,
`network_deny`). Delete the module and each key falls back to `field.unknown`, except a
`harness-overrides.flow` block, which becomes a `harness.unknown` Warning and stays a passthrough. Add a key to it
only with the page's entry that says how to fix it. It covers keys of tables of fields and the
`harness-overrides` namespaces; a model-policy `override` refuses what it does not define, so
its retired keys stay an error that lists the keys it takes. A retired key is not a value: a
value of a key Mars reads (`approval: yolo`) is an ordinary invalid value. The spellings of the
Agent deny list (`tool_policy::DISALLOWED_SPELLINGS`) are also read by the translators, so
they live there and `retired.rs` takes the Agent and Skill tool-policy rows from them and from
`skill::tool_keys()`. The other code that names a key from
outside is a translator from another format (`claude/`, `harness_overrides`, `set_aside`).
