# Provided skills

A host can install its own skills into `.mars/skills/` without adding dependencies or changing the package lock. Sync recognizes provided ownership so a package can replace a provided skill; reference validation separately checks whether a declared provided skill is available on disk.

The unlocked fast path and the locked pass use the same reconciliation plan. It checks every source skill and hashes installed trees: a package sync may have replaced a recorded directory before publishing its lock, and a formerly blocked skill may now be installable. Files are compared as authored, including executable bits.

The provider record authorizes removal or replacement of its own names; an unrecorded different directory is preserved. A package lock claim takes precedence. Before installation, the provider writes a pending record containing old and planned names, then installs and writes the final record. A crash leaves enough ownership information for the next run to reconcile, including removal of a skill the host has since dropped.

Provider names in the record must belong to that provider and be single path components: the record can have been written by another Mars build and must not authorize removal of another provider's files. Readers ignore an unsupported record version; the writer refuses it to avoid overwriting a newer format.
