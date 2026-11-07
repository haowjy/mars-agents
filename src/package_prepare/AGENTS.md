# Package preparation

This is the filesystem adapter for `mars_schema::prepare`; schema classification, discovery, normalization and validation belong to the schema crate. Consumer filters, renames and overlays follow preparation.

`DiskTreeSource` confines reads to the authored root and latches I/O and containment errors because `TreeSource` has no error channel. Take the latched error immediately after preparation. Keep the authored root for relative dependencies and diagnostics even after staging changes the content root; see [resolution](../resolve/AGENTS.md).
