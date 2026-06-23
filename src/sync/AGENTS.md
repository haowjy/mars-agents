# src/sync/ — Sync Engine

Unified sync pipeline orchestration. 10 files, ~6500 lines.

## Mental Model

```
execute()
  ↓
load_config   → ResolvedState → TargetedState → PlannedState → AppliedState → SyncedState → SyncReport
  (Phase 1)      (Phase 2)       (Phase 3)       (Phase 4)       (Phase 5)       (Phase 6)     (Phase 7)
```

### Phase Structs (Move Semantics)

Each phase produces a typed handoff struct consumed by the next — no cloning:

| Phase | Struct | Key Contents |
|---|---|---|
| 1 | `LoadedConfig` | Config, LocalConfig, EffectiveConfig, old_lock, sync_lock |
| 2 | `ResolvedState` | LoadedConfig + ResolvedGraph |
| 3 | `TargetedState` | ResolvedState + TargetState + validation warnings |
| 4 | `PlannedState` | TargetedState + SyncPlan |
| 5 | `AppliedState` | PlannedState + ApplyResult |
| 6 | `SyncedState` | AppliedState + TargetSyncOutcomes + ConfigEntries |

### Resolution Modes

- **Normal**: lock-preferred latest-compatible
- **Maximize**: upgrade to newest versions, optionally bump constraints

### Key Operations

| Function | Responsibility |
|---|---|
| `load_config()` | Acquire sync lock, load config, apply mutations, build effective config |
| `resolve_graph()` | Resolve dependency graph, merge model config from deps |
| `build_target()` | Discover items, detect collisions, rewrite frontmatter refs; stages local items via `crate::staging::stage_local_item` (mod.rs:299–309) |
| `create_plan()` | Diff against lock + disk, generate sync plan |
| `apply_plan()` | Write to `.mars/` canonical store (atomic) |
| `sync_targets()` | Copy to managed target directories (non-fatal per-target) |
| `finalize()` | Write lock, persist model aliases, build report |

## Lossiness Gating

`SyncRequest.lossiness_mode` (mod.rs) selects `LossinessMode::Surface` or `Hidden` when
`execute()` creates the pipeline `DiagnosticCollector`. Lossiness-category diagnostics are
suppressed at emission (`warn_with_category` / `error_with_category`) when mode is `Hidden`.

Only `mars sync` and `mars upgrade` set `Surface`; validate, export, add, repair, etc. set
`Hidden`. `mars check` / `mars init` surface lossiness via the preview path with the same
`LossinessMode::Surface`.

### Frozen Gate

`--frozen` errors if any pending changes would occur. Cannot combine with `Maximize` resolution or config mutations.

### Declaration-Ordered Model Merge

Dependency-ordered alias assembly lives in `src/models/dependencies.rs`.
`sync` calls `crate::models::merged_model_aliases()`/`declaration_ordered_dep_models()`
so compiler + sync share one low-level implementation.

## Patterns

**Dry-run sync:**
```rust
let request = SyncRequest {
    resolution: ResolutionMode::Normal,
    mutation: None,
    options: SyncOptions { dry_run: true, ..SyncOptions::default() },
};
let report = execute(&ctx, &request)?;
```

**Upgrade specific targets:**
```rust
let request = SyncRequest {
    resolution: ResolutionMode::Maximize {
        targets: HashSet::from(["base".into()]),
        bump: true,
    },
    mutation: None,
    options: SyncOptions::default(),
};
```

## See Also

- `src/resolve/AGENTS.md` — dependency resolution (Phase 2)
- `src/compiler/AGENTS.md` — compilation (Phases 3-5)
- `src/target_sync/` — target directory copying (Phase 6)
- `src/target_sync/.context/CONTEXT.md` — per-target ownership, orphan cleanup, collision diagnostics
