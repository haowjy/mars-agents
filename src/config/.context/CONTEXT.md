# src/config/ — configuration schema, link normalization, routing settings

## What's new (PR #51)

`targets.rs` and `routing_settings.rs` are new files alongside the existing config schema (`mod.rs`), load/save logic, and legacy migrations.

## Contracts

- **`targets::normalize_link()` is the single link-normalization function.** Do not implement link normalization logic elsewhere.
- **Only `LinkKind::KnownHarness` links produce `linked_harnesses` that affect routing.** `GenericTarget` (e.g., `.agents`, `agents`, unknown names) and `PathLike` (contains `/` or `\`) links are materialization targets ONLY — they are invisible to the routing engine.
- **`routing_settings::resolve()` converts raw `Settings` → typed routing config.** Both `mars models` and `mars build` must consume this typed result. Do not parse harness strings independently.
- **`Settings::effective_links()` delegates to `targets::effective_links()`**, which resolves `settings.targets` > `settings.managed_root` > empty. This is the single resolution path for link configuration.

## Architecture

### `targets.rs` — link normalization

```
raw target string → normalize_link() → NormalizedLink
                                    → EffectiveLinks (collection)
```

`NormalizedLink` carries: `raw` (original string), `target` (canonical path e.g. `.codex`), `harness` (Some(HarnessId) only for KnownHarness), `kind` (KnownHarness/GenericTarget/PathLike).

Projection methods on `EffectiveLinks`:
- `managed_targets()` — all unique targets (for materialization)
- `linked_harnesses()` — only `KnownHarness` links → `Vec<HarnessId>`
- `linked_harnesses_set()` — `BTreeSet<HarnessId>`

### `routing_settings.rs` — typed routing config

```
Settings → resolve() → ResolvedRoutingSettings
                         ├── harness_order: Option<ParsedHarnessOrder>
                         ├── default_harness: Option<ParsedHarnessValue>
                         ├── linked_harnesses: BTreeSet<HarnessId>
                         └── diagnostics: Vec<RoutingConfigDiagnostic>
```

`ParsedHarnessOrder` captures per-candidate parse results with `HarnessOrderFailure` (Empty/AllInvalid). Invalid harness names produce diagnostics, not errors — the pipeline continues with what's valid.

### `migrations/link.rs` — legacy migration

Distinct from live `targets.rs` logic. Handles historical compat for link format migration. Not used by current routing.

## Rationale

- **Before PR #51**, raw config parsing and harness-name validation happened independently in `models::harness` and `build::policy::harness`, producing divergent diagnostic messages.
- **Typed config boundary:** config parsing produces diagnostics once; consumers receive typed values. This eliminates duplicate validation and inconsistent error messages.
- **LinkKind separation:** keeping KnownHarness/GenericTarget/PathLike distinct ensures routing filtering is precise — a project linking `.agents` (GenericTarget) won't accidentally route only to Claude.
