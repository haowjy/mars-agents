# Surface ownership

Path ownership is scoped to the exact target root and destination. An `Installed` record authorizes deletion and replacement; `PendingDeletion` authorizes only a deletion retry. A pending record must not authorize overwriting content that may have appeared after a failed removal. Explicit `--force` can adopt that content.

Config-entry ownership is separate from file ownership. Hook and MCP entries also require a content match before removal, preserving user edits. See [the implementation](../mod.rs).

Interrupted-write recovery is unresolved for these surfaces. A pre-write claim alone would grant unsafe adoption authority: recovery also needs intent published before output, transaction ordering across surfaces and fault-injection coverage at finalization.
