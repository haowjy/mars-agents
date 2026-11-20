# Models

Resolve model identity first, then assess routes within the configured harness scope: an unrestricted preliminary assessment could run an excluded harness's auth command. Live availability and launch model IDs use the selected routing assessment. See [harness model](.context/CONTEXT.md).

Consumer aliases take precedence over dependencies in declaration order. Builtins are fallback-only: any consumer or dependency alias suppresses them.

A usable catalog stays last-good data after refresh failure. Normal commands return stale data while a detached refresh runs; a cold cache fetches synchronously; offline mode never fetches. The refresh claim and cache revision stop concurrent workers replacing newer data with stale results.

`MARS_OFFLINE` suppresses probes entirely, while `--no-refresh-models` permits reading existing probe caches without running a probe. Don't conflate them. See [probe contracts](probes/.context/CONTEXT.md).

A model's reasoning comes from the models.dev row of the provider its harness calls, never from the project's `catalog_providers` view (`opencode-go` need not be in it). `reasoning::catalog_identity` is the one mapping from a launch id to its catalog model (the provider, and the model with a provider qualification taken off); launch, sync and the listing all go through it, and `routing.catalog_row` is its row. The join reads `ModelsCache::catalog`, the unfiltered snapshot, whose providers stay raw JSON until one is read: take rows from it by provider, never by flattening it. `mars_schema::effort::ModelReasoning` carries what a row says (unknown, does not reason, or reasons with its named levels and whether it can be switched off); the resolver and the listing both read it. A harness's effort levels live in one table, `mars_schema::effort::harness_levels`, which records the version each set was checked against; recheck it when a harness releases.

The catalog is settled once per command, before anything is resolved against it: sync refreshes it before native agents are lowered, not in finalize. Every failed fetch writes the fail marker, so the cooldown holds with or without stale data.
