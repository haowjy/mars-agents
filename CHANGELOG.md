# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Fixed
- OpenCode skills no longer emit a per-skill `model-invocable` dropped-field warning. OpenCode's skill frontmatter has no model-invocation gate, so the axis is structurally unrepresentable; the drop is uniform across every non-model-invocable skill and not actionable by the author. It is now an accepted known target limitation (silent drop) rather than per-item lossiness noise. Genuine OpenCode losses (tool gating, `user-invocable`) still warn.

## [0.10.1-rc.1] - 2026-06-24

### Fixed
- Cursor skill `model-invocable` now maps to native rule modes — `true`/default → Apply Intelligently (`alwaysApply: false` + `description`), explicit `false` → Manual (`alwaysApply: false`, no `description`) — instead of the old `alwaysApply: true` (which forced always-on context) and silent drop on `false`.
- Codex skills with explicit `model-invocable: true` no longer emit a spurious dropped-field warning — Codex is model-invocable by default, so explicit-true loses nothing.
- Lossiness warnings are tiered by behavioral consequence: launch-time/meridian-enforced agent fields (`approval`, `sandbox`, `mode`, `model-policies`, `autocompact`, `fanout`, …) are no longer warned per-agent — they collapse into one summary line, since Meridian applies them at spawn. Genuine target-enforced losses stay loud. `--verbose` restores full per-item detail.
- Inbound lift no longer infers `user-invocable: false` from an OpenCode agent's `mode` (the axes are independent now that `mode` lowers exactly).
- Inbound lift round-trips Cursor Manual skills: an `alwaysApply: false` rule with no `description` restores canonical `model-invocable: false` (and re-emits the Codex `openai.yaml` sibling) instead of erroring.

### Added
- Codex `openai.yaml` sibling (`policy.allow_implicit_invocation: false`) emitted for skills with explicit `model-invocable: false` — closes the Codex skill-invocation gating gap (#116). `LoweredOutput` now carries `siblings` written atomically alongside the primary artifact.
- OpenCode agents emit native `mode: primary|subagent` instead of approximate lossiness.
- `when_to_use` folds into native `description` for Cursor / OpenCode / Codex (which use `description` as the selection hook) instead of being dropped.
- `LossinessMode::Verbose` plus a `--verbose` flag on `sync` / `check` to surface meridian-only/launch-time field detail.

### Changed
- Skill `description` is required only at authored-source validation (MarsNative staging + `mars check`); canonical re-parse tolerates an absent description so lifted Cursor Manual imports stay valid without a fabricated description.
- **Breaking (Cursor output):** Cursor skill `.mdc` frontmatter changes (`alwaysApply`/`description` from the rule-mode + `when_to_use` mapping). Existing Cursor projections are reprojected once on next sync.

## [0.10.0] - 2026-06-24

### Fixed
- Source discovery finds convention `agents/` / `skills/` / `bootstrap/` directories nested below the package root, not only at the top level.
- Duplicate `(kind, name)` discoveries — across convention directories, or a convention item vs a `.claude-plugin` manifest declaration — now raise `DiscoveryCollision` instead of silently keeping one by path order.
- MCP inbound lift: merge `allowed-tools` with `mcpServers` without dropping the allowlist; preserve map-form `tools:` when appending whole-server `mcp(server)` entries.
- Malformed `mcp(...)` tool tokens (e.g. `mcp()`, `mcp(/x)`) are validation errors instead of convention-projected unknown tools — disallowed malformed MCP refs no longer fail open.
- Agent parser: retired top-level `mcp-tools:` / `mcp_tools` emits `RemovedField` diagnostic and staging strips the field from canonical frontmatter (mirrors skills).
- MCP inbound lift: boundary-safe Claude `mcp__` wire parse (no UTF-8 panic on near-prefix tokens); Cursor `Mcp(server:tool)` uses the last `:` as the tool separator so namespaced server ids lift to per-tool `mcp(server/tool)` instead of whole-server; scalar `allowed-tools` / `disallowedTools` strings lift like list entries.
- No-op `mars sync` no longer rewrites linked native skill outputs when projected bytes are already on disk (mtime churn on `.claude/skills/.../SKILL.md` etc.).
- Hook lossiness (`hook-dropped`/`hook-approximate`) now follows the same surface/suppress policy as agent/skill lossiness — hidden on `validate`/`export`/`add` and no longer escalated into a failure by `mars validate --strict`.
- `skill`, `workflow`, and `web` (alias of `web_search`) are recognized tool names — gating them (e.g. `skill(deep-research)`, `Workflow`) no longer emits "not a known tool; passing through verbatim" warnings and projects to native spellings (`Skill(...)`, `Workflow`, `WebSearch`). Recognition only — per-harness enforcement of `disallowed-tools` is unchanged.
- Unknown tools now project via the target harness naming convention instead of passing through verbatim (snake_case custom tools reach Claude/Cursor as PascalCase); MCP wire names (`mcp__…`) are recognized and preserved verbatim without re-casing.

### Added
- Inbound lift of foreign MCP permission tokens to canonical `mcp(server/tool)` on import — Claude `mcp__server__tool` wire refs and Cursor `Mcp(server:tool)` refs in `allowed-tools` / `disallowed-tools` lift without case change; `mcpServers` whole-server entries lift into `tools:` as `mcp(server)`.
- Canonical `mcp(server/tool)` grammar in `tools:` / `disallowed-tools:` — scoped MCP references are parsed and recognized (preserved verbatim, no convention projection or unknown-tool warning); real per-harness emission is a later phase.
- Canonical source staging seam: dependencies resolve through a derived `.mars/staging/` tree with `lift_frontmatter` hook before discovery/hash/apply.
- Per-dialect frontmatter lift tables (Claude/Codex/Cursor/OpenCode) in `staging/lift.rs`; default/inferred Claude lift is idempotent on mars-native packages.
- `when_to_use` on `SkillProfile` with lowering to Claude/Pi and warn-drop on other harnesses.
- `dialect` key on `[dependencies.<dep>]` plus `.opencode`/`.cursor` discovery roots for inbound dialect inference.
- `[skills.<name>]` overlay carriage in config; applied at staging after lift (description, invocability, tool policy).
- Skills `disallowed-tools` canonical denylist.
- Shared `compiler/tool_policy.rs` for agent and skill tool gating (`tools:` list-or-map, `disallowed-tools:`, inline `mcp(...)` grants).

### Changed
- **Breaking:** Source discovery is a single bounded convention walk that finds `agents/` / `skills/` / `bootstrap/` directories (and a root `SKILL.md` fallback) at any depth, grounded to the shallowest package layer (deeper nested containers are ignored). Hidden dot-directories are skipped, so harness output surfaces (`.claude/`, `.codex/`, `.cursor/`, `.opencode/`) are no longer auto-scanned as discovery *sources* — import a foreign hidden layout explicitly via `subpath` + `dialect`. Replaces the previous fixed-top-level scan + container-root heuristic.
- **Breaking:** Removed legacy `mcp-tools:` / `mcp_tools` frontmatter field and `[agents|skills].tools.mcp` overlay key. Author whole-server MCP grants as `mcp(server)` entries in `tools:` (or `tools.allowed` overlays); per-tool grants use `mcp(server/tool)`.
- MCP emission (Phase 4b): `EffectiveToolPolicy` carries structured `mcp_allowed` / `mcp_disallowed` [`McpRef`] values; harness lowering and launch bundles project them via `project_mcp_ref` instead of verbatim `mcp-tools:` strings. Claude agents emit `mcp__…` tokens in `tools:` / `disallowed-tools:` (no `mcp-tools:` field); Claude skills grant MCP into `allowed-tools:` with a lossiness note. Unsupported projections (e.g. Claude `mcp(*/tool)`, Codex/Pi MCP) record lossiness and omit the token.
- Inbound lift: Claude `mcpServers` now appends `mcp(server)` entries to `tools:` instead of lifting to `mcp-tools:`.
- MCP input unification (Phase 2): inline `mcp(...)` entries in `tools:` / `disallowed-tools:` converge on one internal policy model; agent and skill parsers share the same key set.
- Skills use the same canonical tool schema as agents: `tools:` (list or allow/deny map), `disallowed-tools:`, and inline `mcp(...)` MCP grants. Shared parser in `compiler/tool_policy.rs`. Foreign `allowed-tools` lifts to `tools:` at staging.
- Agents honor `user-invocable` (was skills-only).
- Agent `model-invocable` / `user-invocable` a target cannot express now warn (deduped per item×target) instead of silent drop; same for skill tool fields a target cannot carry.
- Lossiness warnings surface only on `mars sync`, `mars upgrade`, `mars init`, and `mars check`; suppressed on validate, export, add, and other sync-pipeline commands.
- `[agents.<name>]` overrides now adjust description, invocation axes, and tool policy (was routing fields only).
- Codex skills: `model-invocable` warn-dropped (Codex reads invocation from sibling policy file, not SKILL.md) instead of emitting ignored `allow_implicit_invocation` frontmatter.
- Authoring foreign flat `allowed-tools`/`disallowed_tools` in a canonical skill now warns (use `tools:`) and the key is stripped from the `.mars/` store instead of leaking into canonical and native output.

## [0.9.0] - 2026-06-22

### Fixed
- Tool policy names now normalize from lowercase, aliases, and PascalCase into Mars snake_case before projecting to each harness native convention.
- `harness-overrides.<harness>` now skips invalid nested values without dropping valid sibling passthrough keys.

## [0.8.12] - 2026-06-21

### Fixed
- `mars agents list` / `mars skills list` now accept their filter flags (`--mode`, `--source`, `--type`, `--model-invocable`). The filters are global, so they work on both the bare form (`mars agents --mode subagent`) and the discoverable subcommand form (`mars agents list --mode subagent`), which previously errored with "unexpected argument".

## [0.8.11] - 2026-06-21

## [0.8.10] - 2026-06-20

### Added
- `mars models prompting <ref>` resolves agent refs before model aliases and shows model prompting guidance, with JSON output for the resolved ref.
- `mars models prompting` accepts `--refresh-models` / `--no-refresh-models` for launch-policy model resolution.

### Changed
- `harness-overrides.<harness>` is now exact target-native passthrough: Mars preserves the matching block in launch-bundle `execution_policy.native_config` without using it to replace top-level Mars fields.

### Fixed
- Mars tool fields now normalize separator-aware aliases like `ask_user` to PascalCase (`AskUser`) before lowering, while preserving unknown tool spellings exactly for the target harness.
- `mars models prompting <agent>` now reports guidance for the launch-resolved runnable model, including overlays, model policies, fallbacks, and model clearing.

## [0.8.9] - 2026-06-19

### Added
- `mars agents list` and `mars skills list` subcommands (aliases for the bare listing commands).

## [0.8.8] - 2026-06-17

### Changed
- Cursor promoted from experimental to first-class launch-bundle target. Removed `is_experimental` flag, `harness_stability` provenance, and user-facing warning. Removed `experimental` field from `HarnessDescriptor`.
- Inventory prompt-file idiom updated from `/tmp/<name>.md` to `$(meridian work path prompts/<name>.md)`.

### Fixed
- Test harness now sanitizes `MERIDIAN_MANAGED` env var, preventing `managed_cmd` output drift when tests run inside a meridian session.

## [0.8.7] - 2026-06-12

### Changed
- Claude target writes hook bindings to `settings.local.json` (gitignored) instead of `settings.json`. Hook commands embed machine-local cache paths that churn on every sync and every machine, so they no longer belong in the committed file. Existing managed hooks in `settings.json` are migrated out on next sync; user-owned hooks are preserved.

## [0.8.6] - 2026-06-12

## [0.8.5] - 2026-06-12

### Changed
- Moved fanout agent list from `[settings.meridian.agent_copy].fanout_agents` to `[settings.meridian.fanout].agents`. Old key parses with a migration warning and is ignored.
- Fanout agents that are native-for-harness now appear in both the Meridian spawn inventory and the native harness agents section.
- Fanout agent list matching is case-insensitive for native emission, consistent with inventory dual-listing.

### Fixed
- `mars build launch-bundle` now surfaces the deprecated `agent_copy.fanout_agents` migration warning in bundle `warnings`.

## [0.8.4] - 2026-06-11

### Added
- `fanout_agents` field in `[settings.meridian.agent_copy]` — selectively enable fanout qualification for listed agents when `include_fanout = false` globally.

## [0.8.3] - 2026-06-07

### Fixed
- Native agent compilation now uses the shared model routing evaluator for native model pinning, resolving auto aliases from `.mars/models-cache.json`, respecting model-policy fallback order, and clearing/skipping target-native model fields when no candidate routes to that harness.

## [0.8.2] - 2026-06-07

### Fixed
- `mars sync` lock rebuild now carries forward existing non-canonical target ownership records while refreshing canonical `.mars` records, instead of relying on a post-build safety-net pass.
- Local package discovery scans only `.mars-src/`, preventing package source `agents/` / `skills/` directories at the project root from being misread as local unmanaged items; `--force` also repairs stale canonical cache files left by the old discovery behavior.

## [0.8.1] - 2026-06-06

### Removed
- `launch_actions` experiment and `mars build launch-bundle --context` / `--transport` flags; launch-bundle schema reverted from v4 to v3 (revival tracked in #101).

### Fixed
- `.mars/native-agents.json` manifest determinism (sorted agent keys via `BTreeMap`) and write ordering (lock persisted before manifest projection).

## [0.8.0] - 2026-06-06
### Changed
- `[settings.agent_copy]` renamed to `[settings.meridian.agent_copy]` (clean break, no shim) — marks selective native copy as meridian-managed-only.
- `mars build launch-bundle --context` / `launch_actions` projection marked EXPERIMENTAL — not consumed by meridian; may be removed.

### Added
- `mars sync` summary now reports native-agent emissions and removals (`emitted N native agents` / `removed N native agents`, with per-line `+/-  <target>/<path> (native agent)`). Previously a managed/SuppressAll sync that pruned native agents printed "already up to date" — the prune was silent. Emissions are diffed against the prior lock so steady-state re-syncs stay quiet; counts also added to `--json`.
- `mars build launch-bundle --context` emits v4 `launch_actions` with subprocess/streaming `kind`, `cwd`, stdin, files, env, and bootstrap/turn protocol shapes for Cursor, Claude, Codex, OpenCode, and Pi.
- `.mars/native-agents.json` manifest after sync/link; launch-bundle inventory splits Meridian spawn agents from harness-native agents.

## [0.7.17] - 2026-06-06

### Fixed
- Harness exhaustion no longer falls through to unlinked `pi`; launch-bundle walks profile model-policy fallbacks instead.

## [0.7.16] - 2026-06-05

### Fixed
- `apply_apply_outcomes_to_lock()` dropped `.claude`/`.cursor`/`.codex` output records on `Updated`/`Installed` actions — `copy_decision()` lost ownership, triggering "not tracked by Mars" warnings every sync.
- `finalize()` dropped non-canonical ownership when native emission failed (I/O, permissions) — carry-forward now preserves old records, respecting explicit removals from reconciliation.

## [0.7.15] - 2026-06-04

### Changed
- Cursor compilation: `sandbox` and `approval` classified as `Approximate` (was `Dropped`). Cursor supports `--sandbox enabled/disabled` and `--force`/`--yolo` CLI flags; runtime projection is in meridian-cli.

### Fixed
- Compilation matrix docs: Cursor sandbox/approval tables and lossiness matrix updated to match code.

## [0.7.14] - 2026-06-02

### Fixed
- Git upgrade hints adapt canonical `github.com/org/repo` identities back to fetchable remotes before `git ls-remote`.

## [0.7.13] - 2026-06-02

### Changed
- `mars sync` upgrade hint now checks locked transitive dependencies, so stale compatible packages pulled through dependency ranges surface before `mars upgrade`.

## [0.7.12] - 2026-05-31

### Added
- `[settings.agent_copy]` — under `MERIDIAN_MANAGED=1` or `agent_emission = "never"`, selectively emit qualifying agents to native harness folders (e.g. `.claude/agents/`) by model→harness binding; `include_fanout` checks profile `model-policies`. Overrides blanket suppression; `agent_emission = "always"` still emits all.

### Changed
- One default harness order (`claude`, `codex`, `pi`, `cursor`, `opencode`) drives provider candidate order and empty-model routing.
- Native agent lifecycle out of `compiler/mod.rs`; one canonical scan; sync + link share post-target lifecycle.
- Native agent compile emits pinned model IDs for model aliases; raw and unpinned aliases still pass through.
- Lock-replay sync skips latest tag lookup unless upgrading or emitting the post-sync upgrade hint.
- `AGENTS.md`: clarify generated `mars.lock` is ignored local state and document `MERIDIAN_TASK_DIR` vs inherited `MERIDIAN_PROJECT_DIR` for nested Meridian commands.

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
