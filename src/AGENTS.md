# mars-agents — src/

Agent package manager. Installs agent profiles and skills from git/local sources into `.mars/` canonical store, then copies managed content into target directories (`.agents/`, `.claude/`, `.codex/`, etc.).

## Mental Model

```
mars.toml + mars.lock (committed, project root)
        ↓ classify and prepare definitions
        ↓ mars sync
    .mars/ (canonical store, gitignored)
        ↓ copy to each target
    targets: .agents/, .claude/, .cursor/ (committed, shared)
```

- **Canonical content is derived, not authored truth.** Fresh clones rebuild `.mars/` content from sources. Retain `.mars/pending-canonical.json` after failed sync: it is write-intent evidence needed to recover ownership (see `sync/AGENTS.md`).
- **Mars never deletes files it didn't create.** Per-target lock ownership — see root `AGENTS.md` Critical Invariants and `target_sync/.context/CONTEXT.md`.
- **All writes are atomic** (tmp+rename). Crash mid-write leaves old file intact.

## Dependency Direction

```
cli ─┬→ sync ─────────→ compiler → target adapters
     └→ build/plugin ─┬→ resolve
                     ├→ sync target preparation
                     └→ compiler lowering
sync/build → resolve ← source
               ↓
            config
               ↓
            models → routing → harness runtime descriptors
```

- `cli/` dispatches commands, finds project root, formats output
- `sync/` orchestrates the pipeline (load → resolve → build → plan → apply → sync targets → finalize)
- `package_prepare` classifies each rooted package before lift or discovery; it rejects invalid/newer schemas and selects schema-v1, legacy-upgrade, or foreign-dialect staging
- `resolve/` resolves dependency graphs with semver constraints; rooted dependencies cross the package-preparation boundary after `apply_subpath`
- `staging/` materializes prepared schema-v1 content before discover/hash, for dependencies and local authored content
- `discover/` owns source item discovery: a bounded convention walk over non-hidden
  package layers for agents, skills, and bootstrap docs; whole hook directories
  are discovered only under the package-root `hooks/`; see
  `src/discover/.context/CONTEXT.md`
- `dialect/` resolves inbound lift dialect per rooted package (explicit `dialect` key > foreign-container path inference > default — Claude for deps, MarsNative for `.mars-src`); declared-package self inputs explicitly use MarsNative; inference does not make hidden containers discovery roots
- `skill_source_name` — single flat-root skill naming rule shared by discovery and staging overlay lookup
- `source/` fetches git/path sources, manages global cache
- `config/` parses mars.toml + mars.local.toml, merges to EffectiveConfig
- `mars-schema` owns pure package-file vocabulary, parsing and validation; callers import it directly
- `compiler/` compiles schema values into target state and handles dual-surface emission
- `target/` per-target compilation adapters (`.claude`, `.codex`, etc.)
- `target_sync/` copies from `.mars/` to configured target directories
- `surface_ownership/` gates linked-target deletes and copy/install on per-target lock records
- `models/` model catalog, alias resolution, derived Possible harness-model rows
- `models/catalog_api.rs` decodes models.dev; `models/catalog_cache.rs` owns
  the last-known-good snapshot and SWR lifecycle;
  `platform::process` owns detached launches for catalog and probe workers
- `curation/` authored display-only filters over Possible; never a routing input
- `routing/` harness candidate evaluation — single evaluator for all routing
- `harness/` runtime harness descriptors and capability snapshot (host); harness ids live in `mars-schema`
- `build/` launch bundle construction and flattened publish-graph plugin builds
- `content` layered lookup for primary and fallback canonical agent / skill files
- `platform::fs` is the canonical layer for durable writes, copies, replacements, comparisons, and removals

## What Changes Together

| Change | Modules |
|---|---|
| New harness | `harness/registry.rs`, `target/` adapter, `compiler/agents/lower.rs` |
| New CLI command | `cli/mod.rs` (enum + dispatch), `cli/<cmd>.rs` |
| Config schema change | `config/mod.rs`, `config/routing_settings.rs` |
| Sync pipeline phase | `sync/mod.rs` (phase struct + function) |
| Model resolution | `models/mod.rs`, `models/availability.rs` |
| Catalog refresh | `models/catalog_cache.rs`, `platform/process.rs` |
| Possible/Curated display | `models/possible.rs`, `curation/` |
| Routing logic | `routing/mod.rs` only — single evaluator invariant |
| Tool capability / native tool name | `crates/mars-schema/src/tool_names.rs` (vocabulary), `crates/mars-schema/src/tool_natives.rs` (per-harness tables + real-tool pin); binary projection remains in `compiler/tool_policy.rs` |

## Anti-Patterns

- **Never add a second candidate evaluator** — `routing::evaluate_candidates()` is the only one. Both `mars models` and `mars build` call it.
- **Never validate harness names outside `mars_schema::harness`** — its parser is the authored-name authority.
- **Never scan-and-delete unknown files in targets** — only remove paths tracked in the lock for that target; see `target_sync/.context/CONTEXT.md`.
- **Never use `eprintln!` in library code** — all diagnostics go through `DiagnosticCollector`.
- **Never hardcode model aliases in the binary** — all aliases come from packages or consumer config.

## Key Invariants

- Windows is first-class. No POSIX-only assumptions in paths, process launching, or filesystem operations.
- No VCS dependency — walk to filesystem root via `Path::parent()`, not `.git` boundaries.
- Execution-policy precedence: CLI > agent overlay > overlay model policy > YAML profile >
  profile model policy > settings model policy > project/local settings > user config > harness
  default. There is no environment-variable policy layer.
- Resolve first, then act — zero mutations if any error detected during resolution.
