# src/models/probes/ — OpenCode & Pi capability probing

## Contracts

- **Probes check binary compatibility at capability-collection time.** Results are cached to disk; commands consume cached results via `CapabilitySnapshot` (see [harness/host.rs](../../harness/host.rs)).
- **`PiProbeResult.compatible == true`** means Pi is installed AND `pi --version` exits 0 AND ALL `PI_REQUIRED_HELP_TOKEN_GROUPS` are present in `pi --help`. Missing any token group → `compatible: false` → routing skips Pi.
- **Probe token groups are arrays of alternatives.** Any token in the group satisfies the requirement. This handles Pi version variation where flags may be renamed or aliased.
- **Cache locations:**
  - OpenCode: `~/.mars/cache/availability/opencode-probe.json`
  - Pi: `~/.mars/cache/availability/pi.json`
- **TTL:** `MARS_PROBE_CACHE_TTL_SECS` env var, default 60s for both probes.
- **Probe timeout:** `MARS_PROBE_TIMEOUT_SECS` env var, default 5s. Applied independently to each subcommand.
- **When offline (`offline: true`) or `allow_probe_refresh: false`:** use stale cache if present; return `Unavailable` if no cache → Pi/OpenCode treated as `Passthrough` by routing.
- **Windows/cache isolation:** Tests that depend on probe cache MUST set `MARS_CACHE_DIR` explicitly to a temp directory. XDG env alone is insufficient on Windows. The cache path is resolved via `crate::platform::cache::global_cache_root()`.

## Architecture

```
probe() / probe_with_timeout()
    ↓
probe_cached(installed, is_offline)   ← runs at snapshot collection time
    ↓
CachedProbeOutcome / CachedPiProbeOutcome
    ├── Hit: fresh cache hit
    ├── Stale: usable but past TTL (triggers background refresh)
    ├── Miss: no cache, ran synchronous probe
    └── Unavailable: offline or not installed
```

Both OpenCode and Pi follow the same pattern:
1. Check if probe should run (not offline, harness installed)
2. Check cache freshness
3. If stale: return stale result + spawn detached background refresh
4. If missing: run synchronous probe, write result to disk

**Background refresh** spawns a detached child process (`mars models __refresh-probe --target {opencode|pi}`) that acquires the same lock and re-runs the probe without blocking the parent.

## Rationale

- **Pi probe token list matches Meridian's `_REQUIRED_HELP_SURFACE_TOKEN_GROUPS_SPAWNED`.** Mars is now the authoritative checker; Meridian trusts Mars `RouteConfidence`.
- **Compatible Pi now routes as `Confirmed` (not `Passthrough`).** This was a key behavioral change in PR #51: Mars now knows Pi is usable, not just installed.
- **Caching avoids re-running probes on every command.** A `pi --help` or `opencode providers list` call takes 1-5 seconds; caching drops this to ~0ms for subsequent commands within the TTL window.

## Patterns

- **Unit tests that don't test Pi routing:** pass `pi_probe_result: None` in `RoutingInput`.
- **Simulate compatible Pi:** `PiProbeResult { compatible: true, ..PiProbeResult::default() }`.
- **Cache path injection in tests:** use `probe_cached_impl()` directly with an explicit path pointing to a temp directory.
