# src/target_sync/

Ownership is scoped to `(target_root, dest_path)`: a `.mars` record does not
authorize mutation under a linked target. A pending deletion authorizes retrying
deletion, but not overwriting an existing path; replacement without `--force`
requires an installed-content claim.

`mars sync` preserves an unmanaged collision with a warning; `mars link` fails.
`--force` adopts it and records the target output in the lock.
