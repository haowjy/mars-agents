# Invariants: how each is kept

The root `AGENTS.md` states each invariant and why users rely on it. This page says how
the code keeps it today, what checks it, and where it falls short. The mechanisms are
current choices; the invariant is what they serve.

## Your files stay yours

**How.** Mars owns a file when `mars.lock` records an installed output for it, or
`.mars/provided.lock` lists it (`src/sync/ownership.rs`, `Ownership::owns`). In linked
targets, a copy proceeds only with an installed checksum claim or `--force`, and a
delete needs a matching record (`src/surface_ownership/mod.rs`). MCP and hook entries
are removed or replaced only when recorded and unchanged (`src/target/mcp_file.rs`,
`hook_file.rs`). A pre-write journal (`.mars/pending-canonical.json`) publishes
ownership to the lock before it is removed. Rules: `docs/internals/lock-file.md`
(Canonical Ownership) and `docs/internals/conflicts.md`.

**Checked by** `tests/self_conflicts.rs`, `tests/config_entries/` (ownership, MCP and
hooks), `tests/sync_recovery.rs`, `tests/sync_behavior.rs`, and the compile-fail test
in `tests/write_capability_compile_fail.rs`: a pending-deletion record exposes no
installed checksum.

**Gaps.** A managed file with only local changes is kept, not replaced
(`src/sync/plan.rs`, `KeepLocal`). Nothing tests the `.mars/provided.lock` half or
`mars provide`. The journal doesn't cover native and config outputs, so an
interrupted write there has no recovery. Three ownership predicates exist on purpose
(`LockIndex`, `Ownership`, `surface_ownership`); don't merge them.

## Only what it was given

**How.** Config comes from exactly three files: `mars.toml`, `mars.local.toml` and the
user `config.toml` (`src/config/user.rs`). A package entry outside the package root is
an error (`src/package_prepare/mod.rs`, `DiskTreeSource::confined`); a subpath can't
leave its checkout (`src/resolve/path.rs`); a symlink is materialized only when its
target stays inside the package (`src/fs/mod.rs`). Destinations are `DestPath`s, which
reject absolute and `..` paths (`src/types.rs`). Archive extraction rejects traversal
and skips symlinks (`src/source/archive.rs`). Per-mechanism docs:
`src/package_prepare/AGENTS.md`, `src/source/AGENTS.md`,
`src/discover/.context/CONTEXT.md`.

**Checked by** unit tests only: `src/source/archive.rs`, `src/discover/tests.rs`
(bootstrap escape), `src/resolve/tests/integration_tests.rs`, `src/types.rs`
(`dest_path_rejects_escape`), `src/cli/mod.rs` (symlinked managed root).

**Gaps.** Nothing tests read confinement in `package_prepare`, the `mars provide --root`
guard, or end to end that a package's writes stay inside the destination.

## Private content stays private

**How.** `mars export` (`src/cli/export.rs`) serializes metadata-only structs: there is
no field a body could go in. Imported MCP server headers that name a credential are
dropped and reported as `mcp.credential-blocked`
(`crates/mars-schema/src/mcp_servers.rs`, `is_credential_variable`; a superset of what
Claude withholds). User docs: `docs/config/mcp-and-hooks.md`.

**Checked by** `tests/validate_and_export.rs`, which plants marker text in a body and
asserts the export doesn't contain it.

**Gaps.** Nothing tests the credential block. `mars export` and `mars validate` are
missing from `docs/cli/commands.md`. "Exported" also means `visibility = exported` for
hooks and MCP servers, a different concept.

## Nothing lost silently

**How.** `src/compiler/lossiness.rs` holds the contract: a dropped restriction prints one
line per project by default; a quiet loss needs a reason and prints only under
`--verbose`. Each target declares what it drops (`src/compiler/agents/lower_policy.rs`),
and one classifier sorts tool losses for every target (`tool_loss.rs`). Rendering:
`src/diagnostic.rs`. User docs: `docs/config/agent-compilation.md`.

**Checked by** `tests/lossiness_surfacing.rs`, and the tests in `lossiness.rs` and
`diagnostic.rs`.

**Gaps.** Nothing tests the `mars build plugin` output. Nothing checks that every
lowering step records what it drops; the per-target tables are maintained by hand.
