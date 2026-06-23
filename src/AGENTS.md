# mars-agents — src/

Agent package manager. Installs agent profiles and skills from git/local sources into `.mars/` canonical store, then copies managed content into target directories (`.agents/`, `.claude/`, `.codex/`, etc.).

## Mental Model

```
mars.toml + mars.lock (committed, project root)
        ↓ mars sync
    .mars/ (canonical store, gitignored)
        ↓ copy to each target
    targets: .agents/, .claude/, .cursor/ (committed, shared)
```

- **`.mars/` is a cache, not the source of truth.** Committed targets + `mars.lock` are the authority. Fresh clone rebuilds `.mars/` from sources.
- **Mars never deletes files it didn't create.** Per-target lock ownership — see root `AGENTS.md` Critical Invariants and `target_sync/.context/CONTEXT.md`.
- **All writes are atomic** (tmp+rename). Crash mid-write leaves old file intact.

## Dependency Direction

```
cli → sync → compiler → target adapters
              ↓              ↑
           resolve ← source  |
              ↓              |
           config ←──────────┘
              ↓
           models → routing → harness registry
```

- `cli/` dispatches commands, finds project root, formats output
- `sync/` orchestrates the pipeline (load → resolve → build → plan → apply → sync targets → finalize)
- `resolve/` resolves dependency graph with semver constraints; runs `staging::stage_rooted_package` after `apply_subpath` when `staging_root` is set
- `staging/` lifts foreign-dialect frontmatter → canonical before discover/hash, in both resolve (dependencies) and sync (local items)
- `dialect/` resolves inbound dialect per package (explicit `dialect` key > foreign-container path inference > default — Claude for deps, MarsNative for local)
- `skill_source_name` — single flat-root skill naming rule shared by discovery and staging overlay lookup
- `source/` fetches git/path sources, manages global cache
- `config/` parses mars.toml + mars.local.toml, merges to EffectiveConfig
- `compiler/` compiles IR into target state, handles dual-surface emission
- `target/` per-target compilation adapters (`.claude`, `.codex`, etc.)
- `target_sync/` copies from `.mars/` to configured target directories
- `surface_ownership/` gates linked-target deletes and copy/install on per-target lock records
- `models/` model catalog, alias resolution, auto-resolve against cached catalog
- `routing/` harness candidate evaluation — single evaluator for all routing
- `harness/` canonical harness vocabulary (registry) + capability snapshot (host)
- `build/` launch bundle construction (serializable artifact for harness runtime)
- `reconcile/` shared atomic fs operations + state-based reconciliation

## What Changes Together

| Change | Modules |
|---|---|
| New harness | `harness/registry.rs`, `target/` adapter, `compiler/agents/lower.rs` |
| New CLI command | `cli/mod.rs` (enum + dispatch), `cli/<cmd>.rs` |
| Config schema change | `config/mod.rs`, `config/routing_settings.rs` |
| Sync pipeline phase | `sync/mod.rs` (phase struct + function) |
| Model resolution | `models/mod.rs`, `models/availability.rs` |
| Routing logic | `routing/mod.rs` only — single evaluator invariant |

## Anti-Patterns

- **Never add a second candidate evaluator** — `routing::evaluate_candidates()` is the only one. Both `mars models` and `mars build` call it.
- **Never validate harness names outside `harness::registry`** — `registry::parse()` and `registry::is_known()` are the only authorities.
- **Never scan-and-delete unknown files in targets** — only remove paths tracked in the lock for that target; see `target_sync/.context/CONTEXT.md`.
- **Never use `eprintln!` in library code** — all diagnostics go through `DiagnosticCollector`.
- **Never hardcode model aliases in the binary** — all aliases come from packages or consumer config.

## Key Invariants

- Windows is first-class. No POSIX-only assumptions in paths, process launching, or filesystem operations.
- No VCS dependency — walk to filesystem root via `Path::parent()`, not `.git` boundaries.
- Config precedence: CLI > ENV > YAML profile > project config > user config > harness default.
- Resolve first, then act — zero mutations if any error detected during resolution.
