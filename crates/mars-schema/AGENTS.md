# mars-schema

This crate reads, validates, translates and edits package data without host I/O. The
binary owns filesystem and network access, orchestration, consumer configuration and
target lowering. Keep the crate usable on `wasm32-unknown-unknown`: the same rules and
serialized results are used by the CLI and JavaScript callers. Share types and rules
across those callers rather than making a second binary-side copy.

## Reading and preparation

- Route authored definitions through the shared reader. Schema-v1 authored fields are
  strict; foreign and pre-v1 definitions go through translation and set-aside so a
  dependency the consumer cannot edit can still install. Keep valid entries in lowered
  fields and network host lists when rejecting individual entries: losing a deny entry
  can widen an agent's permissions. Report lost restrictions as changes.
- Ordinary reading leaves schema-v1 files as written; migration may repair removed
  fields because it writes the result back. Upgrade definitions without changing
  unrelated bytes when possible. Do not change a dependency's `requires-mars` during
  preparation: it gates whether the dependency can be read. Migration adds a missing
  floor when it can edit the manifest minimally; it leaves an existing requirement
  or an inline package table unchanged and provides advice when needed.
- A `mars.toml` without `[package]` is consumer configuration, not a package manifest.
  Use the preparation context and `project_authored_root` to choose the authored tree;
  applying package rules to consumer configuration changes what is read and written.
- Prepare before applying consumer filters and renames. Check skill body links against
  the actual installed layout after those choices: preparation cannot know whether a
  target from another package will be installed. Use the shared item selection result
  for Agents, skills and bootstrap documents so callers agree on what is selected.
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
- Keep an allowed lock when the selection policy prefers it and no request asks for
  latest, without checking whether its tag still exists upstream. This lets a locked
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
