# src/harness/

Two files with a clean boundary: `registry.rs` is pure data; `host.rs` is I/O collected once per command.

## Contracts

### `registry.rs` — canonical harness vocabulary

`harness::registry` is the **only** owner of valid harness identity. No other
module should validate harness names, define provider candidate orders, or
maintain a list of known harness binaries independently.

- `HarnessId`: `Claude | Codex | Pi | OpenCode | Cursor`
- `HarnessClass`: `Native { provider }` (claude↔anthropic, codex↔openai) |
  `ProbeBacked` (pi, opencode) | `UniversalPassthrough` (cursor)
- `parse(name)` / `is_known(name)` — case-insensitive, trim-safe
- `provider_candidate_order(provider)` — canonical evaluation order for a given provider
- `UNKNOWN_PROVIDER_FALLBACK_ORDER` — `[Pi, OpenCode, Cursor]` for unknown/non-native providers
- `native_harness_for_provider(provider)` — only returns `Some` for anthropic→Claude, openai→Codex

**Invariant:** Adding a new harness means one change here (new descriptor in `DESCRIPTORS`)
plus registration in `all()` and `names()`. Nothing else needs updating.

### `host.rs` — capability snapshot

`CapabilitySnapshot` must be collected **once per command invocation** and shared.
Re-collecting mid-command risks probe inconsistency and unnecessary subprocess spawns.

- `collect_capability_snapshot(options)` — collects for all known harnesses
- `collect_capability_snapshot_with_resolver(options, resolver)` — testable variant with injected PATH
- `CapabilityCollectionOptions { offline, probe_refresh }` — `MARS_OFFLINE` and `ProbeRefreshMode` (background / synchronous / skip)
- `ExecutableResolver` trait — cross-platform PATH lookup; `PathExecutableResolver` is the production impl
- `AuthState`: `NotApplicable` (Pi/OpenCode/Cursor), `Authenticated`, `Unauthenticated`, `Unknown`

`CapabilitySnapshot` fields:
- `executable: BTreeMap<HarnessId, ExecutableState>` — PATH lookup result per harness
- `auth: BTreeMap<HarnessId, AuthState>` — auth probe result (only meaningful for Native harnesses)
- `opencode: CachedProbeOutcome` — OpenCode capability probe from disk cache
- `pi: CachedPiProbeOutcome` — Pi capability probe from disk cache

## Architecture

```
registry.rs   ←── pure static data (no I/O, no env reads)
                    HarnessId, HarnessDescriptor, HarnessClass
                    provider orders, normalization, native affinity

host.rs       ←── I/O at collection time only
                    ExecutableResolver → PATH lookup (once per harness)
                    native_auth_state → subprocess probe (claude/codex only)
                    opencode_cache/pi_cache → disk reads
                    → CapabilitySnapshot (cloneable, shared across command)
```

## Rationale

Before this module: harness names and validity were scattered across
`models::harness`, `compiler::agents::HarnessKind`, and hardcoded error strings.
Divergence between modules was a live risk.

`ExecutableResolver` as a trait enables fake PATH injection in tests (`FakeResolver`
with a `HashMap<String, ExecutableState>`) without touching the filesystem.

Windows `.cmd`/`.bat` fallback via the `which` crate lives in
`PathExecutableResolver::resolve` and **only** here. No other module should
implement its own extension-suffix loop.

## Patterns

**Tests:** Inject `FakeResolver` to control which harnesses are "installed"
without real binaries:

```rust
let mut resolver = FakeResolver::default();
resolver.map.insert("pi".to_string(), ExecutableState::Found { path: PathBuf::from("/tmp/pi") });
let snapshot = collect_capability_snapshot_with_resolver(&options, &resolver);
```

**Offline/test options:**

```rust
let options = CapabilityCollectionOptions {
    offline: true,
    probe_refresh: ProbeRefreshMode::Skip,
};
```

This prevents probe refresh and uses stale cache (or returns empty probe result).

**Do NOT** add harness-name validation logic outside this module. Use `registry::parse()` or `registry::is_known()`.
