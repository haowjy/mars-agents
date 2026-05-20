# src/harness/ — canonical harness vocabulary & capability snapshot

## Contracts

- **`registry::parse()` and `registry::is_known()` are the ONLY harness-name authority.** No other module must independently validate or normalize harness names. Any code checking "is this a valid harness?" must go through registry.
- **`registry::provider_candidate_order()` is the canonical candidate list for unknown providers.** The order `Pi → OpenCode → Cursor` is defined once in `UNKNOWN_PROVIDER_FALLBACK_ORDER`. Do not duplicate it.
- **`HarnessClass::Native` means 1:1 provider affinity** (claude↔anthropic, codex↔openai). Auth probing is only meaningful for Native harnesses. Non-native harnesses always return `AuthState::NotApplicable`.
- **`CapabilitySnapshot` must be collected once per command invocation and shared.** Never re-collect mid-command — this causes redundant PATH lookups, auth probes, and cache reads.
- **`ExecutableResolver` is the ONLY owner of cross-platform PATH lookup.** No other module should implement its own `which`/extension-loop logic. Windows `.cmd`/`.bat` fallback lives here and ONLY here.

## Architecture

```
registry.rs          → pure data (no I/O, no env reads)
host.rs              → I/O at collection time only, cached as CapabilitySnapshot
```

`registry.rs` defines `HarnessId` enum, `HarnessDescriptor` (name, binary, default_target, class), and lookup functions. It is a pure-data module — no filesystem access, no environment reads.

`host.rs` performs all I/O: PATH resolution via `ExecutableResolver` trait, auth probing via subprocess (`claude auth status`, `codex login status`), and delegates OpenCode/Pi probe results to `models::probes::*_cache`. Results are frozen into a `CapabilitySnapshot` for the rest of the command to consume.

## Rationale

- **Single ownership of harness identity.** Before PR #51, harness names and validity were scattered across `models::harness`, `compiler::agents::HarnessKind`, and ad-hoc error strings — divergence risk was high. Centralizing in `registry` eliminates drift.
- **`ExecutableResolver` trait enables test injection.** `FakeResolver` (HashMap<String, ExecutableState>) allows tests to control PATH resolution without touching the filesystem.
- **Windows `.cmd`/`.bat` fallback is localized.** Cross-platform executable lookup is a platform-specific concern; the `ExecutableResolver` trait keeps the abstraction clean while `PathExecutableResolver` handles the platform details.

## Patterns

- **Test injection:** Use `FakeResolver` with a `HashMap<String, ExecutableState>` to simulate installed/missing binaries. See `host.rs:221` test module.
- **Offline/testing mode:** Pass `CapabilityCollectionOptions { offline: true, allow_probe_refresh: false }` to skip all I/O probes.
- **DO NOT add new harness-name validation logic anywhere else.** Call `registry::parse()`.
- **DO NOT implement your own `which` or extension loop.** Use `PathExecutableResolver` or inject a custom `ExecutableResolver`.
