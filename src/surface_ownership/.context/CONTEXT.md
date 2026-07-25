# Surface Ownership — Design Rationale

## Why two ownership shapes

Path ownership (`OutputRecord`) and config-entry ownership (`ConfigEntryRecord`)
are structurally different operations. Path ownership is per materialized path
(file or directory): the lock identifies it by `(target_root, dest_path)`.
Config-entry ownership is per entry within a shared target config file: the lock
is keyed by target root and stable config-entry key. Current hook records also
store the emitted JSON so adapters remove only structurally equal entries; the
temporary issue #130 bridge handles older hook records by recorded key and
command path. MCP stale removal uses owned entry keys.

Both obey one contract — Mars may remove only what the lock proves it owns — but
they differ in removal mechanics and in the blast radius of a failure. A failed
or otherwise unconfirmed config-entry removal retains exactly the records whose
removal was not confirmed, while suppressing replacement writes for that whole
`(target_root, surface)` pair. Retention is precise because keeping a record for
an entry already deleted would create ghost ownership. Write suppression is
deliberately conservative and surface-wide because deferring a safe replacement
write until the next sync is preferable to overwriting ownership evidence after
a partial removal.

## Why `RemovalPlan` partitions before mutation

`build()` partitions records by `Surface::of_key` before any removal runs. This
makes cross-surface bleed unreachable rather than merely avoided: the MCP
failure handler receives a `SurfaceRemoval` containing only MCP records, so it
cannot retain Hook records even by mistake.

## Why `WritePermit` binds rather than carries

`bind_config_entries` consumes the permit and rejects a surface mismatch. The
resulting operation derives its target from the permit and carries the checked
payload with it. The alternative — a passive proof token passed alongside the
payload but never consulted — was a real defect found in review: the permit
proved a check had happened somewhere, not that this call was the operation that
had been checked. A capability that is not consumed and bound to its operation
authorizes nothing.

## Why file-output removals stay outside the outcome model

Per-path `OutputRecord` retention is independently correct: a failed delete of
path A retains exactly A's record and does not affect path B. Folding it into the
`(target_root, surface)` outcome would suppress unrelated file writes for no
ownership-correctness gain.
