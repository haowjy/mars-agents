# Package preparation

This module is the binary IO adapter for `mars_schema::prepare`. `DiskTreeSource` exposes an
authored package root lazily, sorts directory entries, confines resolved paths to that root, and
latches IO or containment failures because the host-independent `TreeSource` trait has no error
channel. Always check the latched error immediately after preparation.

`prepare_rooted` runs after dependency `subpath` is applied. It keeps
`RootedSourceRef.authored_root` immutable, reports `Prepared.diagnostics` against authored paths,
parses binary-owned manifest sections from normalized manifest bytes, and returns without
materializing. After the complete graph and local authored packages pass their prepared-diagnostic
gate, `staging` projects the graph. Relative dependency paths use `authored_root` at every depth;
`package_root` is only the later materialized content view.

Commands consume `Prepared` directly:

- check reads typed definition readings and source-keyed diagnostics;
- migrate writes changed normalized bytes to `source_path`;
- sync and plugin resolution retain the result on each graph node;
- local package and `.mars-src` selection compares prepared definitions before overlays.

Do not add binary classification, Agent/skill discovery, lift, upgrade, or schema rereads. Extend
`mars-schema::Prepared` when a consumer lacks semantic information. Consumer filters, renames and
overlays run after preparation.
