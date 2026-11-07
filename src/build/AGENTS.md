# Build

Build produces launch bundles and publishable plugin trees. Plugin builds resolve as a consumer of the package without project-local or host state; launch bundles resolve policy for the current project or an ad-hoc request. See [build contracts](.context/CONTEXT.md).

Bundle warnings describe actionable degraded states. Route provenance, including passthrough and unknown confidence, belongs in routing fields; tool permission gaps belong in rule status and notes. Consumers launch with `routing.harness_model` verbatim: Cursor may already have encoded effort in that value. Candidate slugs are diagnostic only.

Policy fields resolve independently. See [policy](policy/AGENTS.md) for pins, preferences and fallback.
