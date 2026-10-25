# mars-schema

Pure, host-independent schema logic for Mars package files. This crate owns authored
vocabulary, parsing, validation, normalization, and lossless editing. The `mars-agents`
package owns filesystem, network and process I/O, consumer configuration, orchestration,
host capability discovery, and target lowering.

Do not add filesystem access, network clients, process execution, or host-dependent
behavior. Keep `cargo check -p mars-schema --target wasm32-unknown-unknown` green.
Values shared with the runtime are defined here and consumed directly; do not add
duplicate binary-side types or re-export shims.

`package` owns versioned package metadata and manifest classification. `upgrade` is the
single pure pre-v1-to-v1 conversion used by both package preparation and `mars migrate`.
Definition upgrades edit only changed top-level blocks and keep all other authored bytes. A
non-block top-level mapping or failed semantic verification falls back to a reported full
frontmatter rewrite. Unchanged definitions and schema-v1 manifests remain byte-identical. The
fixtures under `fixtures/upgrade/` are the published upgrade contract.

`agent`, `skill`, and `manifest` are the only readers for schema-v1 definitions. Their
serialized reading and diagnostics are pinned by `fixtures/read/`; callers consume these
types directly rather than reading owned frontmatter keys themselves. Unknown fields are
preserved, while removed fields are errors.
