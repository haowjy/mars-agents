# src/sync/

A dry run leaves project state untouched, including staging and recovery. New
write phases need to honor `dry_run`.

Retain `.mars/pending-canonical.json` after failure: it is write-intent evidence
for recovery. Dropping it can turn Mars-written bytes into unmanaged collisions.
