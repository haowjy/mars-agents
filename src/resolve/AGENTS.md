# Resolution

Resolution chooses package versions and prepares their contents before `mars_schema::selection` selects items from the settled graph. Consumers use that selection rather than applying filters again. References deferred to a declared provider are judged without host state; sync reports whether the provider is actually available.

A later constraint can change an earlier version choice, so resolution restarts with the revised choice. Engine-incompatible versions are excluded across restarts; diagnostics must describe the final graph, not a discarded pass. Semver sources can fall back to a compatible version; sources without alternative versions fail instead.

Keep the authored root distinct from the staged package root. Relative dependencies and diagnostics use the authored root even after prepared content is materialized elsewhere. Publication uses `PublishLens`, which excludes local and user configuration so a package is assessed as its consumers see it.

Marketplace entries are untrusted input to git. The schema validates remote source and ref shapes, and the git adapter guards option-like arguments again. Marketplace and plugin content commits have separate lock identities; see [lock ownership](../lock/.context/CONTEXT.md).
