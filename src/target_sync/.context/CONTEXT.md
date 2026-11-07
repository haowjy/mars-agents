# Target sync

Target sync copies canonical content to managed targets and cleans up orphaned outputs. Ownership is scoped to the exact target root and destination; a `.mars` claim does not authorize changes to a linked target. See [surface ownership](../../surface_ownership/.context/CONTEXT.md) for deletion and replacement authority.

Without an installed claim, sync preserves an existing destination and warns; `mars link` fails. Explicit `--force` adopts the destination and records ownership. Failed removals retain their claims so a later sync can retry.
