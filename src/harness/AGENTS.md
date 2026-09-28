# src/harness/ — Harness Identity & Capability Snapshot

Canonical harness vocabulary (`registry`) and one-shot environment collection (`host`). Not routing, not model alias resolution.

## Mental Model

```
registry.rs  →  HarnessId, descriptors, provider orders (pure data, no I/O)
host.rs      →  PATH + probe caches  →  CapabilitySnapshot (clone, share)
```

**Registry owns identity.** Valid harness names, native provider affinity, and evaluation order live only in `registry`. Other modules call `parse()` / `is_known()` — they do not maintain parallel harness lists.
`HarnessId` JSON serializes to registry names (not Rust snake-case variant
names): `OpenCode` is `opencode`, never `open_code`.

**Host collects once.** Routing commands create a lazy `CapabilitySession` at entry.
Live model lists use one `CapabilitySession` through `SessionPossibleSource`;
excluded probe-backed harnesses are not refreshed, even under
`--refresh-models`. The session retains physical executable facts; permission
is not represented by pretending a binary is absent. Do not re-collect
mid-command.

## Authentication evidence

`NativeAuthCache` observes each canonical harness lazily once per command, including
unknown results. Share it across model candidates and live aliases; do not turn
AuthState into a boolean or cache it across commands. Routing checks permission,
installation and support before invoking it. Auth status is not credit/quota proof.
Pi and Cursor successful credential-gated listings imply configured auth only while
their latest cache attempt succeeded. OpenCode's ungated listing never implies auth.

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
