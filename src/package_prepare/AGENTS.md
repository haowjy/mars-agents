# Package preparation

This module is the binary IO adapter for `mars_schema::prepare`. `DiskTreeSource` exposes an
authored package root lazily, sorts directory entries, confines resolved paths to that root, and
latches IO or containment failures because the host-independent `TreeSource` trait has no error
channel. Always check the latched error immediately after preparation.

`prepare_rooted` runs after dependency `subpath` is applied. It calls `mars_schema::normalize` with the dependency's `dialect` as the hint, so a manifest-free dependency (a Claude plugin, a skill tree) arrives as one schema v1 package with relocated output paths and a generated `mars.toml`; a marketplace is an error that lists its plugins until the dependency names one with `plugin`. A dependency rooted at a marketplace plugin (`RootedSourceRef.plugin`, set by the resolver after it fetched the plugin's files) is normalized as that plugin: the stored marketplace entry is the hint, and any marketplace refusal is the plugin's. It keeps
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

Do not add binary classification, Agent/skill discovery, lift, upgrade, or schema rereads. Authored local trees use `mars_schema::prepare` directly. A directory that may be a package or a consumer project (`mars check`, `mars init`) is prepared with `PrepareContext::Project` (`prepare_project`), and the crate decides: a `mars.toml` without `[package]` is consumer configuration, which `prepare` leaves unread (`Prepared.manifest` is `None`). Extend
`mars-schema::Prepared` when a consumer lacks semantic information. Consumer filters, renames and
overlays run after preparation.
