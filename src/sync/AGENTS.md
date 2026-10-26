# src/sync/ — Sync Engine

Unified sync pipeline orchestration.

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
| 1 | `LoadedConfig` | Config, LocalConfig, EffectiveConfig, old_lock, provided names, sync_lock |
| 2 | `ResolvedState` | LoadedConfig + ResolvedGraph |
| 3 | `TargetedState` | ResolvedState + TargetState (canonical items and selected hooks/MCP) + validation warnings |
| 4 | `PlannedState` | TargetedState + SyncPlan |
| 5 | `AppliedState` | PlannedState + ApplyResult |
| 6 | `SyncedState` | AppliedState + TargetSyncOutcomes + ConfigEntries |

### Resolution Modes

- **Normal**: lock-preferred latest-compatible
- **Maximize**: upgrade to newest versions, optionally bump constraints

### Recovery Policy

`SyncRequest.recovery` is strict by default and independent of `--force`.
Upgrade, override, remove, and repair opt into `DeferOnUnreadable` when
staging finds an unreadable removed-schema hook surface.

**Halt gate.** The gate sits between `reader::read` and `compiler::compile`
in `sync::execute`. `unreadable_hook_surfaces` on `ResolvedGraph` has
exactly one consumer: the gate. When `recovery == DeferOnUnreadable` and
the unreadable set is non-empty, the gate persists pending intent mutations
(mars.toml / mars.local.toml, skipped under `--dry-run`), attaches a
`RecoveryHalt` to the `SyncReport`, and returns without entering the
compiler. Strict sync ignores the recorded set and enters the compiler,
which reports the contextualized schema error.

**Invariant:** the sync pipeline assumes the staged tree is total desired
intent. No "frozen" or "exception" category exists in diff, plan, target,
or lock stages. When anything is unreadable the only writes are the user's
own intent files, which are not derived from package content.

**Corrupt-lock repair.** Repair treats a corrupt lock as empty in memory
under the sync flock acquired in `load_config`. The on-disk corrupt bytes
are preserved until a successful full run finalizes via atomic tmp+rename.
A corrupt lock is evidence, not garbage -- replacing it before the pipeline
succeeds would destroy diagnostic information if the run fails.

### Canonical write recovery

`recovery.rs` writes versioned `.mars/pending-canonical.json` before applying new,
absent canonical outputs. It binds exact expected checksums and provenance to the
pre-write lock bytes. Intent is keyed by canonical destination, with current/planned
versions per path, so repeated item moves retain every uncommitted output. The
reader and writer share identity validation; decoded entries use a single-output
PendingWrite type (the journal wire format remains unchanged); the journal path is reserved before
config/output writes, including dry runs. On retry, load validates regular paths (including ancestors)
and recovers matching outputs into the in-memory lock before source selection.
Changed content, symlinks, corrupt intent, and a replaced lock fail closed.

On retry, intent retains the verified current version alongside any planned
replacement until finalization publishes ownership. Prior canonical paths remain
owned until confirmed removal; same-path deletion claims are replaced, not duplicated. No early lock checkpoint:
failed repair must preserve even corrupt lock bytes. Finalization removes intent
after lock publication. Dry-run and resolution failure never publish recovery.
`--frozen` refuses uncommitted recovered claims even when output bytes need no
changes. No-op sync creates no journal. Existing installed claims remain authoritative.
This protects new canonical writes, not native/config emission (#149).
Preexisting outputs selected for explicit force adoption are intentionally not journaled: a later
apply failure leaves the lock unpublished, and the next sync must use `--force` again.

### Key Operations

| Function | Responsibility |
|---|---|
| `load_config()` | Acquire sync lock, load config, apply mutations, build effective config |
| `resolve_graph()` | Resolve dependency graph, merge model config from deps |
| `build_target()` | Build renamed dependency destinations; stage and overlay reader-selected self items (`local_items.rs`); refuse unsafe or wrong-shaped canonical self collisions, while explicit `--force` may replace regular agent files and skill directories; prune unmanaged dependency collisions; rewrite references and validate (`src/sync/validate.rs`) |
| `create_plan()` | Diff against lock + disk, generate sync plan |
| `apply_plan()` | Write to `.mars/` canonical store (atomic) |
| `sync_targets()` | Copy to managed target directories (non-fatal per-target) |
| `finalize()` | Write lock, persist model aliases, build report |

The reader/local-source seam discovers `.mars-src` unconditionally and adds the
current package only with `[package]`. It selects local overrides before staging:
Agents and skills come from prepared schema results, while hooks, MCP servers,
and bootstrap documents keep their binary-owned discovery and materialization.

### Ownership and provided names

`load_config` reads `.mars/provided.lock` under the sync lock into `LoadedConfig.provided`
(`provided::ProvidedNames`; sync never writes it). Two things use it, and nothing else in sync does:

- **Ownership** (`ownership.rs`): every decision about an existing canonical destination (a local
  hook, a local agent or skill, a dependency item in `check_unmanaged_collisions`) asks
  `LoadedConfig::ownership().owns(dest)`: `mars.lock` claims it, or a provider installed it. The
  package item then replaces the provided copy instead of colliding, so a package wins its
  destination on every machine and `mars.lock` is identical with or without a host. Recovery
  (`recovery.rs`) asks a narrower question (was a journaled write already published?) and reads
  `mars.lock` alone; it journals only absent destinations, so replacing a provided directory leaves
  no journal entry. A crash between that replace and the lock write is repaired by `mars provide`
  (foreign bytes under a recorded name are replaced) and re-taken by the next sync.
- **Ref reporting** (`validate::validate_skill_refs`): the one reporter of skill refs against what
  is installed. The resolver never reads provided names and reports nothing for a ref it defers to
  a declared provider; `validate::classify_unresolved_ref` decides `provided-skill-unavailable`,
  `provided-skill-undeclared` or `missing-skill`. `mars check` calls the same classifier without
  provided names, because it is the publish gate.

`TargetState` also carries parsed hooks and MCP servers selected for each managed target. Filtering,
visibility, dependency collisions, and local shadowing therefore choose config entries once for
both installation and config lowering. It separately retains every filtered, exported graph hook, selected or
not; plugin builds read that list for omission diagnostics, and sync never reads it. Hook selection survives an install skip so post-target-sync lowering
can issue the genuine not-installed warning; it carries the selected `TargetItem` hash so the
installed check does not re-hash the source. Config lowering must not rediscover items.

Plugin builds and sync both use `prepare_target_state` for graph collection, optional local
extension, rename application, and validation. MCP discovery warnings enter the diagnostic
collector where discovery fails rather than riding `TargetState`, so every caller sees them.
`RenameIndex::visible_renames` exposes the same own-package-first, then
dependency-declaration-order view per item kind for frontmatter and body references; callers must not
reconstruct that precedence. Renamed skill and agent frontmatter carries the installed name;
the authored source tree remains untouched and the install layer applies the in-memory rewrite.

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
- `src/discover/.context/CONTEXT.md` — filesystem discovery contract and within-source `DiscoveryCollision` rule
- `src/target_sync/` — target directory copying (Phase 6)
- `src/target_sync/.context/CONTEXT.md` — per-target ownership, orphan cleanup, collision diagnostics
