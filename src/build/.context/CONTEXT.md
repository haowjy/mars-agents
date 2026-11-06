# Build contracts

Plugin builds use published package configuration, not local overrides, a lock,
installed state or host model state. Explicit `--out` publishes only at that
destination; a build without `--out` uses `[plugin].dir` and also publishes
generated package-root files listed in `GENERATED_ROOT_FILES`.

Publication swaps the tree before writing root files. The output's
`.mars-plugin.json` sentinel records the previous root hashes so a later build can finish an interrupted publication
without mistaking its own old output for a hand edit. It replaces only recorded
or identical root files. If a nonempty rollback tree lacks a valid sentinel,
recovery preserves both it and the output and refuses to guess.

Fingerprint plugin inputs after recovery, under the output lock and before populating
staging: `inspect` compares the sentinel's input hash to detect stale builds.

Launch-bundle warnings describe actionable degraded states. Route facts such as
passthrough and unknown confidence belong in routing and provenance: passthrough
uses the requested model when no selected route is available, and unknown means
the runnable ID is unconfirmed. These are not themselves actionable failures.
Consumers launch
`routing.harness_model`; candidate slugs are diagnostic evidence.
