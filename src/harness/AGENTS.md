# src/harness/ — Harness Identity & Capability Snapshot

Canonical harness vocabulary (`registry`) and one-shot environment collection (`host`). Not routing, not model alias resolution.

## Mental Model

```
registry.rs  →  HarnessId, descriptors, provider orders (pure data, no I/O)
host.rs      →  PATH + auth + probe caches  →  CapabilitySnapshot (clone, share)
```

**Registry owns identity.** Valid harness names, native provider affinity, and evaluation order live only in `registry`. Other modules call `parse()` / `is_known()` — they do not maintain parallel harness lists.

**Host collects once.** `collect_capability_snapshot(options)` runs at command entry (models list/resolve, launch-bundle policy, etc.). Pass the resulting `CapabilitySnapshot` through the rest of the invocation; do not re-collect mid-command.

## Capability collection

`CapabilityCollectionOptions`:

- `offline` — set from `MARS_OFFLINE` via `models::is_mars_offline()` (env-wide probe gate)
- `probe_refresh` — from `ModelsRefreshControl.probe_refresh` at CLI/build call sites (`Background` / `Synchronous` / `Skip`)

Catalog refresh flags (`--refresh-models`, `--no-refresh-models`) are resolved in `models` — see [../models/AGENTS.md](../models/AGENTS.md). Probe refresh semantics (stale vs miss, background spawn): [../models/probes/.context/CONTEXT.md](../models/probes/.context/CONTEXT.md).

`MARS_OFFLINE` short-circuits probes before cache (`should_probe_*` false). `--no-refresh-models` uses `Skip` while `offline` may still be false — disk-only probe path when the harness is installed. Details in models AGENTS.md.

## Entry Points

- `registry.rs` — `HarnessId`, `HarnessClass`, `provider_candidate_order`, `native_harness_for_provider`
- `host.rs` — `collect_capability_snapshot`, `CapabilitySnapshot`, `ExecutableResolver`

## See Also

- [.context/CONTEXT.md](.context/CONTEXT.md) — registry invariants, snapshot fields, test patterns
- [../models/probes/.context/CONTEXT.md](../models/probes/.context/CONTEXT.md) — Pi/OpenCode/Cursor probe contracts
