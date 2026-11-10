# Resolution

Consumers use `mars_schema::selection`'s result rather than applying filters again. References deferred to a declared provider are judged without host state; sync reports whether the provider is actually available.

A later constraint can change an earlier version choice, so resolution restarts. Engine-incompatible versions stay excluded across restarts, and diagnostics must describe the final graph, not a discarded pass. Semver sources can fall back to a compatible version; sources without alternative versions fail.

Keep the authored root distinct from the staged package root: relative dependencies and diagnostics use the authored root. Publication uses `PublishLens`, which excludes local and user configuration so a package is assessed as its consumers see it.

Marketplace entries are untrusted input to git: the schema validates source and ref shapes and the git adapter guards option-like arguments again. Marketplace and plugin content commits have separate lock identities; see [lock ownership](../lock/.context/CONTEXT.md).
