# Target sync

Target sync copies canonical content to managed targets and cleans up orphaned outputs. Ownership is scoped to the exact target root and destination; a `.mars` claim does not authorize changes to a linked target. See [surface ownership](../../surface_ownership/.context/CONTEXT.md) for deletion and replacement authority.

Without an installed claim, sync preserves an existing destination and warns, `mars link` included (it is a sync). Explicit `--force` adopts the destination and records ownership.

Removal is one primitive, `removal::remove_recorded`, used by `remove_served` (an unnamed target, `mars unlink`), `cleanup_orphans` and the removal of an item the source dropped. It deletes an output only while the hash of what is on disk equals the record's `installed_checksum`. What differs (an edit, a file the user added inside a recorded directory, a tree with a dangling symlink or other non-file, which has no hash) stays: the record is dropped and the target gets one `output-kept` warning. Mars cannot show such an output is its own copy, so it is never deleted. A failed delete retains its claim (a pending deletion, removed without the comparison) so a later sync can retry, and is reported with its path; it does not stop the other removals. A source-wins overwrite of an edited item that still exists upstream is a different path (`resolve_copy`) and is unchanged.
