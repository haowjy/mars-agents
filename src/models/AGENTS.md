# src/models/ — Model Catalog & Alias Resolution

Model aliases, catalog caching, auto-resolve against models.dev API, and dependency-tree merge. 4 files + probes/, ~7000 lines.

## Mental Model

```
[mars.toml] [deps] → merge_model_config() → merged aliases
     ↓                                          ↓
models-cache.json ← fetch_models()   resolve_all_static() → model identity
     ↓                                          ↓
auto_resolve() ← AutoResolve spec          scoped CLI routing
```

### Two Alias Modes

- **Pinned**: `model = "claude-opus-4-6"` — explicit ID, no resolution needed
- **AutoResolve**: `match = ["opus"]` with optional `provider = "Anthropic"` — glob matching against cached catalog, newest release date wins. When provider is omitted, searches across all providers.

### Merge Precedence

consumer > deps (declaration order, first-dep wins) > builtins

Builtins exist for bare convenience (opus, sonnet, haiku, codex, gpt, gemini).
They are used only when both dependency and consumer alias sets are empty;
any configured alias set suppresses the builtin set rather than layering over it.

## Catalog ingest

`fetch_models` keeps only models.dev providers on the catalog allowlist.
Default: `anthropic`, `openai`, `google`, `meta`, `deepseek`, `xai`,
`openrouter`. Override with `[settings] catalog_providers`; `["*"]` keeps
every provider. Pinned aliases do not need this catalog — they resolve
through harness probes.

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

1. Filter by provider (case-insensitive) when specified; skip filter when provider is omitted
2. All match patterns must hit (AND)
3. No exclude patterns may hit (OR)
4. Skip entries ending with `-latest` (synthetic aliases)
5. Sort by newest release_date, then shortest ID, then lexical ID
6. Return first (or all for `auto_resolve_all`)

## Alias Prefix Resolution

`resolve_with_alias_prefix_static()` handles inputs like `opus-4-6` by:
1. Finding the longest matching base alias (e.g., `opus`)
2. Building glob pattern `*{input}*`
3. Matching against all alias filter candidates
4. Returning best match by release date

## Identity Before Routing

Exact, bulk and prefix alias resolution are static: resolve IDs/provider/settings
without executable discovery, auth or support probes. CLI consumers then assess
routes with effective target scope. Never add an unrestricted preliminary routing
pass; a later scoped assessment cannot undo an excluded auth command.

## Launch `harness_model` (argv model id)

After harness selection, `resolve_harness_model()` in `harness_model.rs` produces
`routing.harness_model`. Alias `provider` is **not** a blind `provider/model` prefix:
native Codex/Claude get bare ids when the provider matches; Pi/OpenCode use probe slugs.
Details and examples: [.context/CONTEXT.md](.context/CONTEXT.md).

## Patterns

**Test without real API:**
```rust
let cache = ModelsCache { models: vec![...], fetched_at: None };
let resolved = resolve_all_static(&aliases, &cache);
```

Inject runtime probe/auth evidence at the shared routing evaluator, not model
identity resolution.

## See Also

- [probes/.context/CONTEXT.md](probes/.context/CONTEXT.md) — probe semantics, refresh-mode table, effort slug rules
- [../harness/AGENTS.md](../harness/AGENTS.md) — capability snapshot collection (once per command)
- `src/routing/AGENTS.md` — uses resolved aliases for harness routing
- `src/config/AGENTS.md` — model visibility settings
