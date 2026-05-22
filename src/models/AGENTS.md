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

- `mars models refresh` — fetches from models.dev API, caches to `.mars/models-cache.json`
- `mars models list` — loads dependency aliases from `.mars/models-merged.json`, overlays consumer config, applies visibility filtering
- `mars models resolve <alias>` — resolves against cache

### Cache Behavior

- TTL: 24h default, configurable via `settings.models_cache_ttl_hours`
- Stale fallback: uses existing cache if fetch fails (with diagnostic)
- Cooldown: 5s backoff after failed fetch attempt
- `MARS_OFFLINE=1` forces offline mode

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

- `.context/CONTEXT.md` in `src/models/probes/` — probe semantics
- `src/routing/AGENTS.md` — uses resolved aliases for harness routing
- `src/config/AGENTS.md` — model visibility settings
