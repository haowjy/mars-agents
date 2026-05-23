# src/models/ — Model Catalog & Alias Resolution

Model aliases, catalog caching, auto-resolve against models.dev API, and dependency-tree merge. 4 files + probes/, ~7000 lines.

## Mental Model

```
[mars.toml] [deps] → merge_model_config() → merged aliases
     ↓                                          ↓
models-cache.json ← fetch_models()        resolve_all() → ResolvedAlias
     ↓                                          ↓
auto_resolve() ← AutoResolve spec          harness detection
```

### Two Alias Modes

- **Pinned**: `model = "claude-opus-4-6"` — explicit ID, no resolution needed
- **AutoResolve**: `provider = "Anthropic"`, `match = ["opus"]` — glob matching against cached catalog, newest release date wins

### Merge Precedence

consumer > deps (declaration order, first-dep wins) > builtins

Builtins exist for bare convenience (opus, sonnet, haiku, codex, gpt, gemini) — packages layer descriptions on top.

### No Builtins Invariant

The mars binary ships zero hardcoded model aliases. All aliases come from packages (via `[models]` in their `mars.toml`) or consumer config. Builtins are a minimal fallback layer for out-of-box usability.

## Catalog Lifecycle

- `mars models refresh` — explicit catalog fetch (`RefreshMode::Force`); does not accept refresh flags
- `mars models list` / `mars models resolve <alias>` — merge + resolve; honor `--refresh-models` / `--no-refresh-models`
- `mars sync` — same refresh flags for best-effort catalog refresh before merge write
- `mars build launch-bundle` — same flags via build policy (`models_refresh` on policy input)

Probe subprocess behavior for list/resolve/launch-bundle is tied to the same flags; see [probes/.context/CONTEXT.md](probes/.context/CONTEXT.md) for per-harness probe contracts and cache paths.

### Refresh control (`ModelsRefreshControl`)

CLI flags resolve once via `resolve_models_refresh_control(refresh_models, no_refresh_models)` → `ModelsRefreshControl { catalog_mode, probe_refresh }`. The two flags are mutually exclusive.

| Input | `catalog_mode` (`RefreshMode`) | `probe_refresh` (`ProbeRefreshMode`) |
|---|---|---|
| default | `Auto` | `Background` |
| `--refresh-models` | `Force` | `Synchronous` |
| `--no-refresh-models` | `Offline` | `Skip` |

`RefreshMode` drives `ensure_fresh()` against `.mars/models-cache.json`:

- **Auto** — fetch when TTL stale; stale cache on fetch failure (cooldown/backoff)
- **Force** — always attempt fetch (used by `mars models refresh` and `--refresh-models`)
- **Offline** — disk only; error if no usable cache

`ensure_fresh` coerces **Auto → Offline** when `MARS_OFFLINE` is set (catalog never hits the network). `RefreshMode::Offline` from `--no-refresh-models` uses a distinct error message when cache is missing.

### Cache Behavior

- TTL: 24h default, configurable via `settings.models_cache_ttl_hours`
- Stale fallback: uses existing cache if fetch fails (with diagnostic)
- Cooldown: 5min backoff after failed fetch attempt (`FETCH_FAIL_COOLDOWN_SECS`)
- `MARS_OFFLINE=1` — catalog offline coercion (see above); also sets harness `CapabilityCollectionOptions.offline`

### `MARS_OFFLINE` vs probe `Skip`

Both suppress probe subprocesses, but through different paths:

- **`MARS_OFFLINE`** — host `offline: true`; `should_probe_*` returns false before cache read → probe outcome `Unavailable` even when harness is installed
- **`ProbeRefreshMode::Skip`** (`--no-refresh-models`) — `offline` stays false; installed harnesses still enter probe cache logic but only read disk (stale hit OK, cold miss → `Unavailable`)

Do not conflate env offline with flag-driven skip when debugging missing probe data.

## Auto-Resolve Algorithm

1. Filter by provider (case-insensitive)
2. All match patterns must hit (AND)
3. No exclude patterns may hit (OR)
4. Skip entries ending with `-latest` (synthetic aliases)
5. Sort by newest release_date, then shortest ID, then lexical ID
6. Return first (or all for `auto_resolve_all`)

## Alias Prefix Resolution

`resolve_with_alias_prefix()` handles inputs like `opus-4-6` by:
1. Finding the longest matching base alias (e.g., `opus`)
2. Building glob pattern `*{input}*`
3. Matching against all alias filter candidates
4. Returning best match by release date

## Harness Detection

Resolved aliases include auto-detected harness based on installed binaries and probe results. Encapsulated in `resolve_harness()` — callers don't pass installed harnesses.

## Patterns

**Test without real API:**
```rust
let cache = ModelsCache { models: vec![...], fetched_at: None };
let resolved = resolve_all(&aliases, &cache, &mut diag);
```

**Inject probe results:**
```rust
resolve_all_with_probe(&aliases, &cache, &mut diag, Some(&opencode_probe), Some(&pi_probe));
```

## See Also

- [probes/.context/CONTEXT.md](probes/.context/CONTEXT.md) — probe semantics, refresh-mode table, effort slug rules
- [../harness/AGENTS.md](../harness/AGENTS.md) — capability snapshot collection (once per command)
- `src/routing/AGENTS.md` — uses resolved aliases for harness routing
- `src/config/AGENTS.md` — model visibility settings
