# Lock ownership

Each output is identified by target root and destination. The same source can lower to different bytes on different targets, so canonical diff must compare the `.mars` checksum, and target mutations must use the matching target's claim. See [surface ownership](../../surface_ownership/.context/CONTEXT.md) for the distinction between installed and pending-deletion claims.

Older v2 item checksums cannot distinguish installed content from deletion tombstones. Loading retains dependencies and aliases but drops item and config-entry claims with a warning; unclaimed files remain local until explicitly adopted. Config entries without recorded emitted content are also dropped because Mars cannot distinguish them from user entries.

Marketplace plugin locks record the marketplace and plugin content identities separately. The content commit is replayed only while the marketplace commit and plugin source still match; otherwise the source must be resolved again. This prevents a changed marketplace entry from pinning content from the previous source.
