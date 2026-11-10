# Package preparation

Filesystem adapter for `mars_schema::prepare`; schema classification, discovery, normalization and validation belong to the schema crate. Consumer filters, renames and overlays follow preparation.

`DiskTreeSource` confines reads to the authored root and latches I/O and containment errors because `TreeSource` has no error channel: take the latched error immediately after preparation. Relative dependencies and diagnostics keep the authored root even after staging moves the content root; see [resolution](../resolve/AGENTS.md).
