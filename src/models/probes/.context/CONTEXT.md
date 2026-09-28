# src/models/probes/

Capability probing for OpenCode, Pi, and Cursor harnesses, with disk-backed caching.

## Module layout

| File | Responsibility |
|---|---|
| `mod.rs` | Re-exports; `should_probe_opencode()` / `should_probe_cursor()` guards |
| `probe_refresh.rs` | Shared `ProbeRefreshMode` (background / synchronous / skip) |
| `opencode.rs` | OpenCode probe: provider/model availability via `opencode models` |
| `opencode_cache.rs` | OpenCode probe cache at `{cache_root}/availability/opencode-probe.json` |
| `pi.rs` | Pi probe: binary present + `--version` / `--help` / `--list-models` |
| `pi_cache.rs` | Pi probe cache at `{cache_root}/availability/pi.json` |
| `cursor.rs` | Cursor probe + effort slug resolution (`resolve_cursor_effort_slug`) |
| `cursor_cache.rs` | Cursor probe cache at `{cache_root}/availability/cursor-probe.json` |

## Contracts

### Pi probe semantics

`PiProbeResult.compatible == true` means the version/help surface is usable and
**all** token groups in `PI_REQUIRED_HELP_TOKEN_GROUPS` appear in `pi --help`.
`model_probe_success` independently records whether `pi --list-models` succeeded.

Prerequisites: `pi` on PATH; `pi --version` and `pi --help` exit 0. A failed
`--list-models` does not turn successful help-surface compatibility into incompatibility.
Empty slugs still yield no Pi runnable paths.

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

Probes cache under `{cache_root}/availability/pi.json`,
`opencode-probe.json`, and `cursor-probe.json`. On Linux the default root is
`~/.cache/mars/cache`; `MARS_CACHE_DIR` overrides it.
TTL: `MARS_PROBE_CACHE_TTL_SECS` env var (default 60s).
Probe timeout: `MARS_PROBE_TIMEOUT_SECS` (default 5s).

The lazy `CapabilitySession` reads each cache on first harness access and memoizes
the outcome for the command. Refresh behavior is controlled by `ProbeRefreshMode`
on `CapabilityCollectionOptions`:

| Mode | Stale usable | Miss / unusable |
|---|---|---|
| `Background` (default) | Return stale + spawn `mars models __refresh-probe` | Sync probe in-process |
| `Synchronous` (`--refresh-models`) | Sync probe in-process (no spawn) | Sync probe in-process |
| `Skip` (`--no-refresh-models`) | Return stale, no spawn | Unavailable |

`MARS_OFFLINE` disables probe subprocesses entirely (`should_probe_*` returns false).

Stale usable cache is still returned under `Skip` when the harness is installed — only refresh is
suppressed.

A failed synchronous or background attempt preserves the last good listing for
all three probes, including Pi. Cache outcomes expose `latest_attempt_ok`;
failed attempts keep `fetched_at`, advance `last_attempt_at` beyond it and
record `last_error`. Pi/Cursor routing uses their last-good slugs for support,
but `ListingFailed` rather than listing-implied auth until a later success.
Background refresh is asynchronous: the first stale command can use the prior
auth flag; the next command sees the failed refresh.

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
let pi_probe = PiProbeResult {
    compatible: true,
    model_probe_success: true,
    ..PiProbeResult::default()
};
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
