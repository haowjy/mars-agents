# src/lock/ — Lock Ownership Registry

`src/lock/` owns the `mars.lock` schema, migration, persistence, and lookup
views. Treat the lock as Mars's ownership registry: it records which logical
items Mars manages, which outputs were materialized, and which target root owns
each output path.

## Mental Model

Lock v3 separates a logical item from its path-scoped lifecycle claims:

```text
items."skill/planning"
  source_checksum          # source-tree content
  outputs[]
    target_root = ".mars"  # canonical store output
    dest_path = "skills/planning"
    state = installed      # checksum asserts installed bytes
    installed_checksum

After an unconfirmed removal, the output instead has
`state = pending-deletion` and no checksum. It retains deletion authority only.
```

The same `dest_path` may appear under multiple `target_root`s. Native targets
can compile the same skill into different bytes, so `.mars/skills/foo` and
`.pi/skills/foo` are different ownership records even though their relative
paths match.

## Key Rules

- **Target root is part of output identity.** Mutation, diff, collision, and
  carry-forward paths must use `(target_root, dest_path)` lookups.
- **Canonical sync diff is `.mars`-scoped.** Comparing `.mars/` disk content to
  a `.pi`, `.codex`, or `.opencode` checksum creates false local-modified
  warnings.
- **Unscoped `dest_path` views are broad views only.** Use them for listing,
  diagnostics, or legacy compatibility, not for deciding whether a concrete
  target path is owned or unchanged.
- **State mars cannot interpret is dropped, never inferred.** A v2 lock keeps only its
  dependencies and aliases, and a config-entry record without `emitted_json` is dropped,
  each with one load warning; neither fails the sync or deletes anything.
- **Marketplace plugins extend `LockedSource`, not the version.** `plugin` and `content {url,
  subpath, commit}` are optional and skipped when absent, so locks without plugins read and write
  unchanged at version 3. A plugin dependency's `url`, `version` and `commit` stay the marketplace's
  own; `content` is where the plugin's files came from (see `.context/CONTEXT.md`).
- **A hook item records `source_path`**, its directory under the package root (`hooks/guard`, `/`
  separated), optional and skipped when absent so the lock stays version 3. `sync/kept_hooks.rs`
  finds a kept hook's install by package + path; locks without it fall back to the names the
  broken `hook.toml` still declares. Older Mars ignores the field and drops it when it rewrites the
  lock; the next current-Mars sync records it again.
- **Do not change the lock schema for lookup fixes.** `LockIndex` is the
  ephemeral seam for efficient read shapes over the persisted nested-output schema.
- **Keep path comparisons separator-tolerant.** `DestPath` uses forward-slash
  canonical form; lookup normalization preserves Windows compatibility.

## Anti-Patterns

- Keying lock outputs only by `dest_path` when target-specific checksums matter.
- Treating a `.mars` output as authorization to mutate a linked/native target.
- Scanning target directories and deleting unknown paths.
- Routing library warnings directly to stderr instead of diagnostics.

## Entry Points

- `mod.rs` — schema types, load/write, lock build, `LockIndex`.
- `.context/CONTEXT.md` — target-scoped lookup contracts and rationale.
- `../target_sync/.context/CONTEXT.md` — linked-target mutation ownership rules.
