# src/models/probes/

Capability probing for OpenCode, Pi, and Cursor harnesses, with disk-backed caching.

## Module layout

| File | Responsibility |
|---|---|
| `mod.rs` | Re-exports; `should_probe_opencode()` / `should_probe_cursor()` guards |
| `probe_refresh.rs` | Shared `ProbeRefreshMode` (background / synchronous / skip) |
| `opencode.rs` | OpenCode probe: provider/model availability via `opencode models ls` |
| `opencode_cache.rs` | OpenCode probe cache at `~/.mars/cache/availability/opencode-probe.json` |
| `pi.rs` | Pi probe: binary present + `--version` / `--help` / `--list-models` |
| `pi_cache.rs` | Pi probe cache at `{cache_root}/availability/pi.json` |
| `cursor.rs` | Cursor probe + effort slug resolution (`resolve_cursor_effort_slug`) |
| `cursor_cache.rs` | Cursor probe cache at `~/.mars/cache/availability/cursor-probe.json` |

## Contracts

### Pi probe semantics

`PiProbeResult.compatible == true` means probe subprocesses succeeded and **all** token groups in
`PI_REQUIRED_HELP_TOKEN_GROUPS` appear in `pi --help` output (after stream merge below).

Prerequisites: `pi` on PATH; `pi --version` and `pi --help` exit 0. `pi --list-models` must exit 0
for a full probe; its output fills `model_slugs` (routing / `mars models list` Pi paths) but does
**not** set `compatible` — empty slugs with `compatible: true` still yield no Pi runnable paths.

**Stream merging:** probe subprocesses use stdout when non-empty after trim; otherwise stderr.
Pi 0.75.x experimental builds emit `--help`, `--version`, and `--list-models` on stderr only.
Older Pi builds that print to stdout are unchanged.

A single missing token group → `compatible: false` → routing engine skips Pi
(records `skip_reason: "pi_incompatible"`).

Token groups are arrays of alternatives: any token in the group satisfies the group.
Example: `&["--session-dir", "PI_CODING_AGENT_SESSION_DIR"]` — either token satisfies.
This handles Pi version variation without requiring exact string matches.

**When Pi probe is absent** (offline, stale cache, probe disabled): routing engine
treats Pi as `Passthrough` (installed but capability unknown). This is safe — Pi
may still work, but we cannot confirm compatibility.

### OpenCode probe semantics

`OpenCodeProbeResult` records provider presence and model slugs available in the
OpenCode installation. `Likely` confidence requires positive provider + model match.

### Cursor effort resolution

Cursor often exposes the default effort tier as an **unsuffixed** slug (e.g. `gpt-5.5`), not
`gpt-5.5-medium`. Mars maps `medium`, `none`, `auto`, and `default` to that base slug when it
exists in the probe catalog; otherwise effort resolution fails closed (`NoEffortMatch`).

Launch-bundle applies the resolved slug to `routing.harness_model` and clears
`execution_policy.effort` when resolution succeeds. Claude slugs prefer `-thinking-` variants when
multiple matches exist at the same effort tier.

### Cache

Probes cache under `~/.mars/cache/availability/{pi,opencode,cursor}-probe.json`.
TTL: `MARS_PROBE_CACHE_TTL_SECS` env var (default 60s).
Probe timeout: `MARS_PROBE_TIMEOUT_SECS` (default 5s).

Cache is read at `collect_capability_snapshot()` time. Refresh behavior is controlled by
`ProbeRefreshMode` on `CapabilityCollectionOptions`:

| Mode | Stale usable | Miss / unusable |
|---|---|---|
| `Background` (default) | Return stale + spawn `mars models __refresh-probe` | Sync probe in-process |
| `Synchronous` (`--refresh-models`) | Sync probe in-process (no spawn) | Sync probe in-process |
| `Skip` (`--no-refresh-models`) | Return stale, no spawn | Unavailable |

`MARS_OFFLINE` disables probe subprocesses entirely (`should_probe_*` returns false).

Stale usable cache is still returned under `Skip` when the harness is installed — only refresh is
suppressed.

### Windows/test cache isolation

Tests that exercise probe caching or depend on deterministic cache state **must**
set `MARS_CACHE_DIR` explicitly to a temp directory. XDG env vars (`XDG_CACHE_HOME`)
are not honored on Windows, so tests relying on them produce non-deterministic
results on Windows. `MARS_CACHE_DIR` is cross-platform safe and takes precedence
over platform-specific cache discovery on all platforms.

```rust
// In test setup:
std::env::set_var("MARS_CACHE_DIR", temp_dir.path());
```

## Rationale

Pi probe token list (`PI_REQUIRED_HELP_TOKEN_GROUPS`) matches Meridian's
`_REQUIRED_HELP_SURFACE_TOKEN_GROUPS_SPAWNED`. Mars is now the authoritative
checker; Meridian trusts Mars route confidence and skips its own probe when
`route_confidence` is `confirmed` or `likely`.

Before PR #51, Pi was always `Passthrough` in Mars routing regardless of whether
Pi actually supported the required flags. This meant Mars could route to Pi, and
Meridian would only discover incompatibility at launch time. The probe moves
detection earlier.

Caching: `pi --help` runs once per TTL, not on every `mars models` or
`mars build launch-bundle` invocation. The 60s TTL balances freshness with
subprocess overhead for commands that run `mars` repeatedly.

## Patterns

**Unit test without real Pi binary:**

```rust
let pi_probe = PiProbeResult { compatible: true, ..PiProbeResult::default() };
// Inject Some(&pi_probe) into RoutingInput — no subprocess needed
```

**Test with incompatible Pi:**

```rust
let pi_probe = PiProbeResult {
    compatible: false,
    help_surface_tokens_missing: vec!["--mode | rpc".to_string()],
    ..PiProbeResult::default()
};
```

**Skip probes in offline test scenarios:**

```rust
let options = CapabilityCollectionOptions {
    offline: true,
    probe_refresh: ProbeRefreshMode::Skip,
};
let snapshot = collect_capability_snapshot_with_resolver(&options, &resolver);
// snapshot.pi will be Unavailable → Passthrough in routing
```
