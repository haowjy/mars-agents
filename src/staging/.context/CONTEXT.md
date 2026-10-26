# Prepared package materialization

`staging` is a filesystem projection of `mars_schema::Prepared`. It does not classify, discover,
lift, upgrade, or validate Agent and skill definitions.

For a resolved dependency, `materialize_prepared_rooted` projects the prepared manifest,
definitions and listed payload into an owned scratch directory, then copies only the binary-owned
hook, MCP, bootstrap and foreign-manifest surfaces before applying consumer skill overlays. It does
not copy unrelated authored files. `RootedSourceRef.authored_root` remains the dependency origin;
only `package_root`, the materialized content view, changes.

For local content, `materialize_local_prepared` writes one selected definition and its crate-listed
payload under `_local/<item>`. Flat-skill exclusions are consumer policy and apply during this copy.

All scratch work completes before the diagnostic gate. Durable publication copies the scratch tree
to `.mars/.staging.pending` and atomically replaces `.mars/staging`. The next holder of `sync.lock`
removes that exact pending path after an interruption; no temporary-name glob is reclaimed.

`overlay.rs` is the remaining semantic-looking code here because `[skills.<name>]` is consumer
configuration. It must run after schema preparation and uses the installed name after dependency
renames.
