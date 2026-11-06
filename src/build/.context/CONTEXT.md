# Build contracts

Plugin builds use published package configuration, not local overrides, a lock,
installed state or host model state. Explicit `--out` publishes only at that
destination; configured builds also plan package-root files.

Publication swaps the tree before writing root files. The sentinel records the
previous root hashes so a later build can finish an interrupted publication
without mistaking its own old output for a hand edit. It replaces only recorded
or identical root files and preserves unowned rollback evidence.

Fingerprint inputs after recovery, under the output lock and before populating
staging, so crash leftovers cannot make a clean checkout appear stale.

Launch-bundle warnings describe actionable degraded states. Route facts such as
passthrough and unknown confidence belong in routing and provenance: treating
expected route metadata as a warning obscures real problems. Consumers launch
`routing.harness_model`; candidate slugs are diagnostic evidence.
