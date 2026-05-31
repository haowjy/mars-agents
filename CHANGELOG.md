# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added
- `[settings.agent_copy]` — under `MERIDIAN_MANAGED=1` or `agent_emission = "never"`, selectively emit qualifying agents to native harness folders (e.g. `.claude/agents/`) by model→harness binding; `include_fanout` checks profile `model-policies`. Overrides blanket suppression; `agent_emission = "always"` still emits all.

### Changed
- One default harness order (`claude`, `codex`, `pi`, `cursor`, `opencode`) drives provider candidate order and empty-model routing.
- Native agent lifecycle out of `compiler/mod.rs`; one canonical scan; sync + link share post-target lifecycle.
- Native agent compile emits pinned model IDs for model aliases; raw and unpinned aliases still pass through.
- Lock-replay sync skips latest tag lookup unless upgrading.

### Fixed
- First-sync native ownership seeds from apply outcomes.
- `mars link` SuppressAll native reconcile scoped to linked harness only.

## [0.7.11] - 2026-05-30

## [0.7.10] - 2026-05-30

## [0.7.9] - 2026-05-30

## [0.7.9-rc.1] - 2026-05-30

## [0.7.8] - 2026-05-29

## [0.7.7] - 2026-05-29

### Fixed
- `mars list` / `mars list --status` / `mars why` / `mars doctor` showed every item N× (once per configured target). Used `canonical_flat_items()` for catalog views, `flat_items_for_target()` for divergence checks.
- Overlay model override (`mars.local.toml`) with incompatible provider now pivots to a compatible harness instead of hard-failing. E.g. profile `harness: codex` + overlay `model: sonnet` → routes to `claude` harness.
- Harness constraint error messages now include actionable fix suggestions.

## [0.7.6] - 2026-05-28

### Changed
- `AutoResolve` model aliases no longer require `provider` when `match` is specified. When provider is omitted, resolution searches across all providers in the models cache.

### Fixed
- Cursor routing: when probe slugs don't contain the model but `provider_constraint` says `cursor`, return `Constrained` evidence instead of hard-rejecting with `no_model_match`. Stale probe cache no longer routes cursor models to Pi.

## [0.7.5] - 2026-05-28

### Fixed
- Launch-bundle fixed-harness routing now soft-fails `no_model_match` only when the harness comes from a higher-precedence source than the selected model (for example CLI harness overriding profile model), clears the model to passthrough routing, and emits a warning instead of failing hard.

## [0.7.4] - 2026-05-27

### Added
- `effort: none` accepted as valid sentinel in agent frontmatter and model-policy overrides — means "no effort level" (same as omitting the field). Previously errored with invalid value.

### Fixed
- Cursor effort resolution now falls back to bare slug for models that have no effort-suffixed variants at all in the probe list (e.g. bare `composer`), instead of erroring with `NoEffortMatch`.

## [0.7.3] - 2026-05-26

### Changed
- Builtin model aliases now act as an empty-project fallback only; any dependency, project, or local model alias suppresses them so package-provided alias sets stay noise-free.
- Added a shared layered config boundary for project + project-local overlays (`user < project < project-local < command`) and switched launch-bundle policy resolution plus `mars models` routing to consume the same effective settings view.
- `mars.local.toml [settings]` now overlays the full settings surface using typed merge semantics (scalar replace, map key replace, array replace) instead of a routing-only subset.
- Config layering helpers moved into `src/config/layering.rs`, with explicit replace-by-key overlay semantics for keyed `[models]` and `[agents]` blocks.
- Runtime config consumers (`build` policy resolution, `mars models`, and `mars doctor`) now reuse a single effective project-config load path per command and share model-alias merge helpers instead of reloading/hand-merging config at multiple call sites.
- Added command-scoped lazy harness capability sessions for Pi/OpenCode/Cursor probe checks, memoized per harness and consumed by `mars models resolve` and launch-bundle routing so candidate evaluation only probes harnesses that are actually assessed.
- `mars models list` is now static by default (alias/catalog metadata only). Use `mars models list --live` for routed harness + availability details.

### Fixed
- `mars models resolve` passthrough success output no longer emits noisy catalog warnings when routing evidence is `confirmed` or `constrained`.
- `mars models list|resolve`, build policy routing, sync cache refresh, and validate compatibility checks now read settings from the merged effective project config (including `mars.local.toml`), and models commands return local-config parse/validation errors instead of silently defaulting.
- `mars sync` and native agent generation now honor `mars.local.toml [models]` overlays, and dependency alias conflict diagnostics are suppressed when a local model alias owns that name.
- `mars sync` now persists dependency alias winners in committed `mars.lock` (`dependency_model_aliases`) and no longer uses `.mars/models-dependencies.json` as alias authority.
- Raw model-id resolution and launch-bundle routing continue to honor `mars.local.toml` `harness_order` overlays while using lazy probe lookups, so local routing precedence remains consistent without eagerly probing unrelated harnesses.
- Cursor effort routing now reports typed failure causes (probe unavailable/empty, missing model prefix, missing effort variant), supports Composer bare-slug fallback when no effort variant exists, and emits precise launch-bundle errors instead of warning-string heuristics.

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
