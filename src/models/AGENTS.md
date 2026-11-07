# Models

Model identity resolution is independent of harness discovery and authentication. Resolve identity first, then assess routes within the configured harness scope; an unrestricted preliminary assessment could run an excluded auth command. Live availability and launch model IDs must use the selected routing assessment instead of repeating support selection. See [harness model](.context/CONTEXT.md).

Consumer aliases take precedence over dependencies in declaration order. Builtins are fallback-only: any consumer or dependency aliases suppress them. Curation affects display, not identity or routing.

A usable catalog remains last-good data after refresh failure. Normal commands return stale data while a detached refresh runs; a cold cache fetches synchronously. Offline mode never fetches. The refresh claim and cache revision prevent concurrent workers from replacing newer data with stale results.

`MARS_OFFLINE` suppresses probes entirely, while `--no-refresh-models` permits reading existing probe caches without running a probe. Do not conflate them. See [probe contracts](probes/.context/CONTEXT.md).
