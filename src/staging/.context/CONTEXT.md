# Prepared package materialization

`staging` is a filesystem projection of `mars_schema::Prepared`. It does not classify, discover,
lift, upgrade, or validate Agent and skill definitions.

For a resolved dependency, `materialize_prepared_rooted` projects the prepared manifest,
definitions and listed payload into an owned scratch directory, then copies only the binary-owned
hook, MCP, bootstrap and plugin-manifest surfaces (the `bootstrapDocs` declaration) before applying consumer skill overlays. It does
not copy unrelated authored files. `RootedSourceRef.authored_root` remains the dependency origin;
only `package_root`, the materialized content view, changes. A manifest-free dependency materializes in the schema v1 layout `normalize` gave it: `skills/<slug>/`, `agents/<slug>.md`, and the generated `mars.toml`.

For local content, `materialize_local_prepared` writes one selected definition and its crate-listed
payload under `_local/<item>`. Flat-skill exclusions are consumer policy and apply during this copy.

Graph and local-package prepared diagnostics gate before scratch materialization begins. Binary
surface and compiler diagnostics then gate durable publication. A real sync builds the generation in
place at `.mars/.staging.pending` (a dry run builds it in a temporary directory outside the project
and publishes nothing), and publication renames that path over `.mars/staging`; nothing is copied.
The next holder of `sync.lock` removes that exact pending path after an interruption; no
temporary-name glob is reclaimed.

`overlay.rs` is the remaining semantic-looking code here because `[skills.<name>]` is consumer
configuration. It must run after schema preparation and uses the installed name after dependency
renames.
