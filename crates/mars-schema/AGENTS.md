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
Changed definition frontmatter is re-emitted; unchanged definitions and schema-v1 manifests
remain byte-identical. The fixtures under `fixtures/upgrade/` are the published upgrade contract.
