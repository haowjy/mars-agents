# src/target_sync/

Copies compiled content from `.mars/` to configured target directories and performs
per-target orphan cleanup. Ownership rules live in `src/surface_ownership.rs` and
are shared with native reconcile (`src/compiler/mod.rs`) and `mars link`.

## Contracts

### Per-target lock ownership

Lock v2 `OutputRecord` entries carry `target_root` + `dest_path`. Mars may delete,
remove-on-`Removed`, or overwrite a path in a linked target **only** when the
previous lock contains a matching record for that exact pair.

**Invariant:** A path tracked only under `.mars` does **not** authorize mutation
under `.cursor`, `.claude`, or any other target root.

Use `LockFile::contains_output(target_root, dest_path)` at mutation sites.
Orphan cleanup scopes to `output_dest_paths_for_target(target_root)` — never
`all_output_dest_paths()` joined against every target.

### `surface_ownership` — shared gating

| Function | Use |
|---|---|
| `may_delete(old_lock, target_root, dest_path)` | Orphan cleanup, Removed deletes, native reconcile deletes |
| `copy_decision(...)` | Copy/install when dest already exists |
| `warn_unmanaged_collision(...)` | Preserve untracked collision; hint correct `--force` command |
| `warn_unmanaged_adopted(...)` | `--force` took ownership; lock will record the target output |

`CollisionAdoptHint` selects the diagnostic hint:
- `SyncForce` → suggests `mars sync --force`
- `LinkForce` → suggests `mars link <target> --force`

### Unmanaged collision semantics

When dest exists on disk but the lock has no `(target_root, dest_path)` record:

| Command | Default | `--force` |
|---|---|---|
| `mars sync` | Preserve local file; emit `target-unmanaged-collision` | Overwrite/adopt; emit `target-unmanaged-adopted`; record lock |
| `mars link` | Fail (exit 2); emit `target-unmanaged-collision` | Adopt; persist linked-target outputs in lock |

### Lock finalization after target sync

`sync_managed_targets` returns `synced_outputs` and `removed_dest_paths` per target.
`finalize()` merges these via `apply_target_sync_outputs` and
`apply_compiled_native_outputs` so linked-target records persist in `mars.lock`.

`lock.canonical_flat_items()` is for `.mars`-only views (e.g. `mars link` seed
state). Do not use bare `dest_path` iterators for linked-target ownership checks.

## Diagnostic codes

| Code | Meaning |
|---|---|
| `target-unmanaged-collision` | Untracked existing file preserved; run suggested `--force` to adopt |
| `target-unmanaged-adopted` | `--force` adopted an untracked collision; lock updated |

## Consumers

```
surface_ownership.rs  ←── single rule set
        ↑
target_sync/mod.rs    ← orphan cleanup, Removed, copy/install
compiler/mod.rs       ← reconcile_native_agent_surfaces, dual_surface_compile
cli/link.rs           ← link collision fail/adopt + lock persist
sync/mod.rs           ← passes old_lock + CollisionAdoptHint::SyncForce
```

## Regression tests

- `sync_preserves_handwritten_cursor_agents_when_lock_only_tracks_mars` (integration)
- `sync_preserves_handwritten_collision_when_lock_only_tracks_mars` (unit)
- `link_fails_on_unmanaged_collision_without_force` / `link_force_adopts_unmanaged_collision_and_records_lock` (integration)

Manual smoke: `tests/smoke/manual/target-scoped-linked-targets.md`
