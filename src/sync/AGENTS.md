# src/sync/

A dry run leaves project state untouched, including staging and recovery. New
write phases need to honor `dry_run`. Canonical ownership recovery runs in
memory and reports `sync-recovered`; staging recovery is skipped.

Retain `.mars/pending-canonical.json` after failure: recovery matches its expected
checksums and lock identity to written bytes, restoring ownership in memory.
Remove it only after publishing the lock, as `recovery::complete` does.
