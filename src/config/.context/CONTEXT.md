# src/config/

Config loading, schema, and routing config boundaries.

## Module layout

| File | Responsibility |
|---|---|
| `mod.rs` | `mars.toml` schema (`Settings`, `Config`), load/save, `EffectiveConfig` |
| `targets.rs` | Link normalization — raw target strings → typed `NormalizedLink` / `EffectiveLinks` |
| `routing_settings.rs` | Raw `Settings` → typed routing input with shared diagnostics |
| `migrations/link.rs` | Legacy link normalization (historical compat for old config forms) |

## Contracts

### `config::targets` — link normalization

`normalize_link(raw)` is the **single** link-normalization function.
Do not duplicate link-parsing logic elsewhere.

`LinkKind` is the critical distinction for routing:

| Kind | Examples | Effect on routing |
|---|---|---|
| `KnownHarness` | `claude`, `.claude`, `codex`, `.codex`, `opencode`, `.opencode`, `pi`, `.pi`, `cursor`, `.cursor` | Produces `linked_harnesses` — filters routing candidates |
| `GenericTarget` | `agents`, `.agents`, `.foo`, unknown names | Materialization target only — **invisible to routing** |
| `PathLike` | `path/to/dir`, `C:\foo` | Materialization target only — **invisible to routing** |

`NormalizedLink.harness: Option<HarnessId>` — `Some` only for `KnownHarness` links.
`EffectiveLinks.linked_harnesses()` returns only the `Some` entries.

**Invariant:** Adding `.agents` to `settings.targets` must never change harness routing.
Only explicitly-named harness targets (with or without leading `.`) affect routing.

`effective_links(targets, managed_root)` resolves the legacy `managed_root` fallback:
- `targets` takes precedence when present
- `managed_root` is used only when `targets` is absent
- Both None → empty `EffectiveLinks` (no link constraints)

### `config::routing_settings` — typed routing config

`resolve(settings)` converts raw `Settings` into `ResolvedRoutingSettings`.
Both models commands and build policy must consume this typed result — they must
not re-parse `harness_order`/`default_harness` strings independently.

`ResolvedRoutingSettings` fields:
- `harness_order: Option<ParsedHarnessOrder>` — None if not set in config
- `default_harness: Option<ParsedHarnessValue>` — None if not set or invalid
- `linked_harnesses: BTreeSet<HarnessId>` — derived from `effective_links()`
- `diagnostics: Vec<RoutingConfigDiagnostic>` — invalid names, empty order, etc.

Invalid harness names in `harness_order` or `default_harness` produce diagnostics
(not hard errors) so the command can continue with degraded config and warn the user.

## Rationale

Before PR #51: raw config parsing and harness-name validation happened independently
in `models::harness` and `build::policy::harness`, producing divergent diagnostic
messages. `targets.rs` and `routing_settings.rs` are the shared boundary that
prevents this.

The `migrations/` subdirectory holds read-compat normalization for legacy config
forms written by older versions of mars. It is separate from the live
`targets.rs` to avoid coupling live routing logic to historical migration logic.

## Patterns

**Link inspection:**

```rust
let links = config::targets::effective_links(settings.targets.as_deref(), settings.managed_root.as_ref());
let linked = links.linked_harnesses(); // Vec<HarnessId> — routing constraints only
let targets = links.managed_targets(); // Vec<String> — all materialization paths
```

**Routing settings:**

```rust
let routing = config::routing_settings::resolve(&settings);
// Pass to routing::evaluate_candidates via RoutingInput:
//   settings_harness_order: routing.harness_order_names().as_deref()
//   config_default_harness: routing.default_harness_name().as_deref()
//   linked_harnesses: routing.linked_harness_names().as_deref()
```
