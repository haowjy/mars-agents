# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Changed
- Added a shared layered config boundary for project + project-local overlays (`user < project < project-local < command`) and switched launch-bundle policy resolution plus `mars models` routing to consume the same effective settings view.
- `mars.local.toml [settings]` now overlays the full settings surface using typed merge semantics (scalar replace, map key replace, array replace) instead of a routing-only subset.
- Config layering helpers moved into `src/config/layering.rs`, with explicit replace-by-key overlay semantics for keyed `[models]` and `[agents]` blocks.
- Runtime config consumers (`build` policy resolution, `mars models`, and `mars doctor`) now reuse a single effective project-config load path per command and share model-alias merge helpers instead of reloading/hand-merging config at multiple call sites.

### Fixed
- `mars models resolve` passthrough success output no longer emits noisy catalog warnings when routing evidence is `confirmed` or `constrained`.
- `mars models list|resolve`, build policy routing, sync cache refresh, and validate compatibility checks now read settings from the merged effective project config (including `mars.local.toml`), and models commands return local-config parse/validation errors instead of silently defaulting.
- `mars sync` and native agent generation now honor `mars.local.toml [models]` overlays, and dependency alias conflict diagnostics are suppressed when a local model alias owns that name.

## [0.7.2] - 2026-05-24

### Fixed
- `resolve_harness_model` no longer prepends `{provider}/{model_id}` from alias `provider` before harness logic; native Codex/Claude receive bare model ids, Pi/OpenCode use probe slugs (fixes `gptmini` → `openai/…` on Codex and Pi).

## [0.7.1] - 2026-05-24

## [0.7.0] - 2026-05-23

### Fixed
- Canonical sync diff uses `.mars` lock outputs, avoiding false local-modified warnings when linked targets store different compiled checksums for the same item path.

## [0.6.6-rc.1] - 2026-05-23

### Changed
- Cursor native agent lowering now emits Cursor-specific markdown: one-line normalized `description`, `skills` passthrough, and policy-field lossiness preserved.
- Removed user-facing `[models.<alias>.native]` model override config.
- Cursor native agent `model` now uses the shared Cursor effort resolver against cached probe slugs (with conservative fallback to original token when no probe/candidate match applies).

## [0.6.5] - 2026-05-23

### Changed
- Launch-bundle auto-routing defaults to `harness_order = ["claude", "pi", "codex", "opencode", "cursor"]` when `settings.harness_order` is unset (explicit empty/invalid still falls through).
- Native harness routing (`claude`, `codex`) compares the requested model id against the cached models.dev catalog (same slug matching as Pi/OpenCode), not only alias/provider affinity.
- `mars models list` and `mars models resolve` now use the same routing evidence assembly as launch-bundle, including cached catalog slugs for native harness matching.
- Cursor probe prefix matching now requires an exact match or hyphen boundary, avoiding ambiguous prefix matches like `gpt-5` matching `gpt-55-*`.
- Bare agent model tokens infer `provider_for_order` from model id prefixes (e.g. `claude-opus-4-6` → `anthropic`).
- Linked-harness fallback walks `harness_order` and skips harnesses already rejected (`pi_incompatible`, `no_model_match`, …) instead of always selecting the first linked target.
- Auto-routing defers `Passthrough` harness selection until later candidates are evaluated, so native catalog matches (e.g. Codex for OpenAI models) win over earlier universal harness passthrough.
- Launch-bundle resolves Cursor `model + effort` into the exact probe slug as `harness_model` and clears `execution_policy.effort` when applied; Claude thinking variants are preferred when multiple slugs match.
- Cursor effort `medium`, `none`, `auto`, and `default` resolve to the unsuffixed base slug when the probe lists it (Cursor’s default tier), instead of requiring a `-medium` suffix.
- `routing.candidate_slugs` is diagnostic-only; consumers should run `harness_model` verbatim.
- `--refresh-models` on `mars models list|resolve`, `mars sync`, and `mars build launch-bundle`: force models.dev catalog refresh and run harness probes synchronously (no background `__refresh-probe` spawn on stale cache).
- `--no-refresh-models` on `mars build launch-bundle` matches models/sync: disk-only catalog and probe `Skip` (stale probe cache still used when present).
- `build launch-bundle` calls `ensure_fresh` for the models.dev catalog (default `Auto`: HTTP only when TTL stale) instead of read-only `load_models_cache`.

## [0.6.4] - 2026-05-23

### Added
- Cursor probe: `cursor agent --list-models` probe backed routing. Cursor changes from `UniversalPassthrough` to `ProbeBacked`; cursor models show `availability: runnable` when probe succeeds.
- Cursor probe cache: TTL cache (`cursor-probe.json`) with stale/miss/hit/unavailable outcomes and background refresh via `mars models __refresh-probe --target cursor`.
- Cursor prefix routing: `candidate_slugs` in launch bundle routing section carries all catalog slugs matching the requested model prefix; supports Python-side effort resolution.
- `classify_cursor` in availability layer: `CursorProbe`, `CursorProbeNegative`, `CursorProbeUnknown` sources in `AvailabilitySource`.
- Graceful degradation: no probe result, probe failure, or empty catalog all fall through to passthrough behavior (same as previous `UniversalPassthrough`).

### Changed
- Cursor harness changes from `HarnessClass::UniversalPassthrough` to `HarnessClass::ProbeBacked` in registry.

## [0.6.3] - 2026-05-23

### Fixed
- Pi probe reads CLI output from stderr when stdout is empty (Pi 0.75+), fixing false `pi_incompatible` and empty `model_slugs` in routing and `mars models list`.
- Pi `--list-models` space-separated tables with extra columns (context, max-out, …) now parse provider/model from the first two columns.

## [0.6.2] - 2026-05-22

## [0.6.1] - 2026-05-22

### Changed
- Launch-bundle model now optional; unset model routes to installed/default harness and leaves harness model empty for harness defaults.
- Default to RC release when no release label present on merged PR (previously skipped release entirely).

### Fixed
- Linked-target sync no longer deletes or overwrites hand-written files when the lock only tracks the same path under `.mars`. Orphan cleanup, Removed handling, and copy paths now require a per-target `OutputRecord`. Pre-existing untracked collisions are preserved with `target-unmanaged-collision`; `mars sync --force` and `mars link --force` adopt them and record ownership (`target-unmanaged-adopted`). Closes #60.
