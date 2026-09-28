# src/models/ — Model Catalog & Alias Resolution

Model aliases, catalog caching, derived Possible rows, auto-resolve against
models.dev API, and dependency-tree merge. See `probes/` for harness caches.

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
- `mars models aliases` / `mars models resolve <alias>` — merge + resolve; honor `--refresh-models` / `--no-refresh-models`
- `mars sync` — same refresh flags for best-effort catalog refresh before merge write
- `mars build launch-bundle` — same flags via build policy (`models_refresh` on policy input)

Probe subprocess behavior for list/resolve/launch-bundle is tied to the same flags; see [probes/.context/CONTEXT.md](probes/.context/CONTEXT.md) for per-harness probe contracts and cache paths.

`possible.rs` provides a derived, non-persisted harness×model projection over
this catalog and the existing probe caches. `SessionPossibleSource::rows_for`
loads one installed, permitted harness lazily; `all_rows` collects the display
inventory. It never reads aliases or curation. Probe-backed rows carry listing
time, last-error, auth-gated and latest-attempt provenance; native rows are
inferred from catalog providers. See [possible.rs](possible.rs).
`listing_issues` exposes both no-last-good failures and retained listings whose
latest refresh failed from the same session observation, once per harness for
human diagnostics; an unavailable row is never fabricated.

Authored `mars.curated.toml` display rules live in `src/curation/` and are not
imported by model resolution, routing, or launch-bundle policy. Only the
`mars models list` renderer projects Possible through Curated.

### Refresh control (`ModelsRefreshControl`)

CLI flags resolve once via `resolve_models_refresh_control(refresh_models, no_refresh_models)` → `ModelsRefreshControl { catalog_mode, probe_refresh }`. The two flags are mutually exclusive.

| Input | `catalog_mode` (`RefreshMode`) | `probe_refresh` (`ProbeRefreshMode`) |
|---|---|---|
| default | `Background` | `Background` |
| `--refresh-models` | `Force` | `Synchronous` |
| `--no-refresh-models` | `Offline` | `Skip` |

`RefreshMode` drives `ensure_fresh()` against `.mars/models-cache.json`:

- **Background** — fresh data returns immediately; stale usable data returns immediately and starts detached `models __refresh-catalog`; cold or unusable cache fetches synchronously
- **Synchronous** — internal worker mode; rechecks freshness under the cache lock before fetching and never starts another worker
- **Force** — synchronous fetch regardless of cache age (used by `mars models refresh` and `--refresh-models`)
- **Offline** — disk only; error if no usable cache

`ensure_fresh` coerces every mode to **Offline** when `MARS_OFFLINE` is set (catalog never hits the network). `RefreshMode::Offline` from `--no-refresh-models` uses a distinct error message when cache is missing. The hidden worker receives its project root, cache path, refresh interval, provider allowlist, generation, and claim token as arguments; it uses null stdio, no shell, and the cache lock/freshness recheck. It cannot recurse. Only this internal command can bypass project discovery for an ad-hoc root; the cache path must still match that root. A reaper thread waits for the child in long-lived callers without blocking the stale read.

### Cache Behavior

- No hard read expiry: a nonempty valid catalog is last-known-good data, even after the refresh-after interval
- Refresh-after: 24h default, configurable via `settings.models_cache_ttl_hours`; `0` makes every normal command eligible to trigger a background refresh
- A failed/empty refresh retains the last-good catalog and stores the failure reason for later diagnostics
- Cooldown: 5min backoff after failed fetch attempt (`FETCH_FAIL_COOLDOWN_SECS`)
- `RefreshOutcome::Stale` reports `spawned`, `already_in_progress`, `cooldown`, or `spawn_failed`; it never claims the asynchronous fetch succeeded. `peer_refreshed` means another worker completed between the reader's initial cache read and claim check.
- A separate atomic claim and short-lived claim lock coalesce worker launches without waiting on the network/cache-write lock. Before writing a claim, readers recheck generation, cache freshness, live claim, and failure cooldown under this lock and return that cache snapshot. The worker removes only its own token; an expired 120-second lease recovers crashes. The models.dev HTTP call has a 60-second global deadline (DNS through body, across redirects), leaving a minute for worker startup, parsing, and cache writes.
- Successful writes advance `.models-cache.generation` under the cache lock; workers recheck their observed generation so even `refresh-after = 0` coalesces concurrent fetches
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

CLI routing reuses the same longest-base lookup for provider constraints and harness
preference. Identity resolution itself remains probe-free.

## Identity Before Routing

Exact, bulk and prefix alias resolution are static: resolve IDs/provider/settings
without executable discovery, auth or support probes. CLI consumers then assess
routes with effective target scope. Never add an unrestricted preliminary routing
pass; a later scoped assessment cannot undo an excluded auth command.

`provider_constraint_for_alias()` supplies the shared authored restriction for launch
and standalone routing: explicit provider first, then a provider-qualified pinned
model. Provider inference may use the model family, never the preferred harness.

## Launch `harness_model` (argv model id)

After harness selection, `resolve_harness_model()` in `harness_model.rs` projects
the **selected assessment** into the launch ID and provider used by
`routing.harness_model` and live availability.
Alias `provider` is **not** a blind `provider/model` prefix: native Codex/Claude
preserve the requested spelling; probe-backed harnesses use the selected slug.
Details and examples: [.context/CONTEXT.md](.context/CONTEXT.md).

Live availability in `availability.rs` projects the selected routing assessment.
It must not perform its own provider/model support check: Pi/OpenCode slugs can
cross the alias's inferred provider, and Cursor can accept a provider constraint
without a matching cached slug.

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
- `src/config/AGENTS.md` — model settings; `src/curation/AGENTS.md` — display-only curation
