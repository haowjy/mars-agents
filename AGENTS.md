# Development Guide: mars-agents

Mars is an agent package manager. It installs agent profiles and skills from git/local sources into a `.mars/` cache, then copies managed content into target directories (`.agents/`, `.claude/`, `.cursor/`, etc.).

## Critical Invariants

### Mars does NOT own target directories

`.agents/`, `.claude/`, `.codex/`, `.cursor/` can contain hand-written content that mars didn't create. These are shared directories — mars writes into them but doesn't own them.

**Mars must NEVER delete files it didn't create.** Orphan cleanup must only remove files whose `dest_path` was in the previous `mars.lock` but not in the current sync result. Never scan-and-delete-unknown.

The lock file (`mars.lock`) is the authority on what mars manages. If it's not in the lock, mars doesn't touch it.

### Atomic writes

Config, lock, and installed files use temp + rename. Crash mid-write leaves the old file intact. All target copies use the `reconcile` layer's atomic operations.

### Resolve first, then act

Validate + build full desired state before mutating files. If any conflict or error is detected during resolution, zero mutations occur.

### No heuristics

User intent is expressed through explicit flags and arguments, not inferred from string patterns.

## Architecture

```
mars.toml + mars.lock (committed, project root)
        ↓ mars sync
    .mars/ (cache, gitignored)
        ↓ copy
    targets: .agents/, .claude/, .codex/, .cursor/ (committed, shared)
```

- **`.mars/`** — mars's working cache. Gitignored. Rebuilt by `mars sync` from lock + sources. Contains resolved content (`agents/`, `skills/`), merge base cache (`cache/bases/`), models cache (`models-cache.json`), sync lock.
- **Target directories** — mars copies managed items into these. They may contain non-mars content. Mars tracks what it put there via the lock. Configured via `settings.targets` in mars.toml, defaults to `[".agents"]`.
- **`.mars/` is NOT the source of truth** — it's a cache. The committed targets + `mars.lock` are the authority. On fresh clone (no `.mars/`), `mars sync` rebuilds the cache from sources.

## Sync Pipeline

Orchestrated in `src/sync/mod.rs` with typed phase functions (move semantics):

```
load_config → resolve_graph → build_target → create_plan → apply_plan → sync_managed_targets → finalize
```

1. **load_config** — acquire sync lock, load mars.toml + local overrides, apply mutations
2. **resolve_graph** — resolve dependency versions, merge model aliases from dep tree
3. **build_target** — discover items (agents/skills) from all sources including local package, detect collisions
4. **create_plan** — diff against lock, generate sync plan
5. **apply_plan** — write resolved content to `.mars/` (atomic writes)
6. **sync_managed_targets** — copy from `.mars/` to each configured target (atomic copies, no orphan nuke)
7. **finalize** — write lock, build report

### Key types

- `SourceOrigin::Dependency(name)` / `SourceOrigin::LocalPackage` — where an item came from
- `InstallDep` — consumer install intent (mars.toml `[dependencies]`)
- `ManifestDep` — package export (required URL, no path deps)
- `Materialization::Copy` / `Materialization::Symlink` — how items land in `.mars/`

## Model Catalog

`mars models` commands manage model metadata:

- `mars models refresh` — fetch from models.dev API, cache to `.mars/models-cache.json`
- `mars models list` — show aliases (builtin + config)
- `mars models resolve <alias>` — resolve against cache

Two alias modes:
- **Pinned**: `model = "claude-opus-4-6"` — explicit ID
- **Auto-resolve**: `match = ["opus"]`, `provider = "anthropic"` — glob matching against cache, newest wins

Builtin aliases: opus, sonnet, haiku, codex, gpt, gemini. Lowest priority, overridable.

## Key Modules

| Module | Responsibility |
|---|---|
| `src/sync/` | Pipeline orchestration + typed phases |
| `src/target_sync/` | Copy from `.mars/` to managed targets |
| `src/reconcile/` | Shared atomic fs operations (Layer 1) + item reconciliation (Layer 2) |
| `src/models/` | Model catalog, auto-resolve, cache, builtin aliases |
| `src/config/` | mars.toml + mars.local.toml schemas, load/save, merge |
| `src/lock/` | mars.lock schema, load/write, ownership tracking |
| `src/resolve/` | Dependency + version resolution and graph ordering |
| `src/source/` | Source fetching (git + path) and global cache |
| `src/discover/` | Discover agents/skills by filesystem conventions |
| `src/diagnostic.rs` | Structured diagnostics (no eprintln! in library code) |
| `src/cli/` | Clap args, root discovery, command dispatch, output |

## Dev Workflow

```bash
cargo build
cargo test
cargo clippy
```

Integration tests under `tests/`. Prefer keeping changes localized to one module.

## Releasing

```bash
mars version patch --push    # bump, commit, tag, push → triggers CI
```

The `v*` tag triggers GitHub Actions to build and publish to PyPI, npm, and crates.io.
