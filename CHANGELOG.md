# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Changed
- Launch-bundle auto-routing defaults to `harness_order = ["claude", "pi", "codex", "opencode", "cursor"]` when `settings.harness_order` is unset (explicit empty/invalid still falls through).
- Native harness routing (`claude`, `codex`) compares the requested model id against the cached models.dev catalog (same slug matching as Pi/OpenCode), not only alias/provider affinity.
- `mars models list` and `mars models resolve` now use the same routing evidence assembly as launch-bundle, including cached catalog slugs for native harness matching.
- Cursor probe prefix matching now requires an exact match or hyphen boundary, avoiding ambiguous prefix matches like `gpt-5` matching `gpt-55-*`.
- Bare agent model tokens infer `provider_for_order` from model id prefixes (e.g. `claude-opus-4-6` → `anthropic`).
- Linked-harness fallback walks `harness_order` and skips harnesses already rejected (`pi_incompatible`, `no_model_match`, …) instead of always selecting the first linked target.
- Auto-routing defers `Passthrough` harness selection until later candidates are evaluated, so native catalog matches (e.g. Codex for OpenAI models) win over earlier universal harness passthrough.
- Launch-bundle resolves Cursor `model + effort` into the exact probe slug as `harness_model` and clears `execution_policy.effort` when applied; Claude thinking variants are preferred when multiple slugs match.
- `routing.candidate_slugs` is diagnostic-only; consumers should run `harness_model` verbatim.

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
