# Models

Resolve model identity first, then assess routes within the configured harness scope: an unrestricted preliminary assessment could run an excluded harness's auth command. Live availability and launch model IDs use the selected routing assessment. See [harness model](.context/CONTEXT.md).

Consumer aliases take precedence over dependencies in declaration order. Builtins are fallback-only: any consumer or dependency alias suppresses them.

A usable catalog stays last-good data after refresh failure. Normal commands return stale data while a detached refresh runs; a cold cache fetches synchronously; offline mode never fetches. The refresh claim and cache revision stop concurrent workers replacing newer data with stale results.

`MARS_OFFLINE` suppresses probes entirely, while `--no-refresh-models` permits reading existing probe caches without running a probe. Don't conflate them. See [probe contracts](probes/.context/CONTEXT.md).

A model's reasoning comes from the models.dev row of the provider its harness calls, never from the project's `catalog_providers` view (`opencode-go` need not be in it): `reasoning::catalog_row` is the one join per harness, over `ModelsCache::catalog`, the unfiltered snapshot. A harness's effort levels live in one table, `mars_schema::effort::harness_levels`, which records the version each set was checked against; recheck it when a harness releases.
