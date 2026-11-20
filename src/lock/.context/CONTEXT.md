# Lock ownership

Each output is identified by target root and destination. The same source can lower to different bytes on different targets, so canonical diff must compare the `.mars` checksum, and target mutations must use the matching target's claim. See [surface ownership](../../surface_ownership/.context/CONTEXT.md) for the distinction between installed and pending-deletion claims.

Older v2 item checksums cannot distinguish installed content from deletion tombstones. Loading retains dependencies and aliases but drops item and config-entry claims with a warning; unclaimed files remain local until explicitly adopted. Config entries without recorded emitted content are also dropped because Mars cannot distinguish them from user entries.

Marketplace plugin locks record the marketplace and plugin content identities separately. The content commit is replayed only while the marketplace commit and plugin source still match; otherwise the source must be resolved again. This prevents a changed marketplace entry from pinning content from the previous source.

A sync plans, writes and removes only the records of the targets the config names (`.mars` always). `LockFile::set_aside_unmanaged` takes every other target's outputs and config entries out of the lock a sync reads, `restore_unmanaged` puts them back into the one it writes, and each such target is one `unmanaged-target` warning per sync; only `mars unlink` removes them. This is one rule for a hand-edited `targets` and for a target Mars stopped supporting alike.

An item with no outputs is gone, except a hook on a managed target that takes none of it (`hook/<name>@<target>`): it has nothing to write there, and its record is what tells the next sync the loss is not new. `link` and `unlink` do not call `finish`: every item keeps its `.mars` output through them, and only a sync knows which hook records stay. Only `LockFile::finish` prunes, once, after every output change; a reducer that removes an output must not prune items, or it drops that record on any sync that removes a file. `finish` also names `_self` under `[dependencies]` whenever an item has that source.
