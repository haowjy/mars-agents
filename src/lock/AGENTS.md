# Lock

The lock is the ownership registry for package outputs. A logical item can have different bytes at the same relative path in different targets, so ownership, diff and mutation checks must use both target root and destination. An unscoped destination lookup cannot authorize a concrete target mutation.

A failed deletion retains retry authority without an installed-content claim. Older state that cannot prove ownership is dropped with a warning rather than inferred. See [lock contracts](.context/CONTEXT.md) and [surface ownership](../surface_ownership/.context/CONTEXT.md).

Keep destination comparisons separator-tolerant for Windows. Use the read index to change lookup behavior without needlessly changing the persisted schema.
