# Lock

The lock is the ownership registry for package outputs. The same logical item can have different bytes at the same relative path in different targets, so ownership, diff and mutation checks must use both target root and destination: an unscoped destination lookup cannot authorize a concrete target mutation.

A failed deletion retains retry authority without an installed-content claim. Older state that cannot prove ownership is dropped with a warning, never inferred. Destination comparisons stay separator-tolerant for Windows. Change lookup behavior through the read index, not the persisted schema.

See [lock contracts](.context/CONTEXT.md) and [surface ownership](../surface_ownership/.context/CONTEXT.md).
