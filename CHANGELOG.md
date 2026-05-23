# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

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

## [0.6.0] - 2026-05-22

### Changed
- Launch-bundle tool projection now warns unknown tool names only in `tools.allowed` for first-class harnesses; unknown `tools.disallowed` entries are dropped silently.
- Claude tool registry covers full Claude Code tool surface: Cron, AskUser, Notifications, PlanMode, Worktree, LSP, Monitor, SendUserFile, ScheduleWakeup, RemoteTrigger, ToolSearch, and Task sub-tool aliases.

## [0.5.0] - 2026-05-22

## [0.4.8] - 2026-05-22

### Changed
- Shared route slug matching for routing + availability; Pi probe cache schema v2 invalidates legacy no-slug entries.
- Models CLI now consumes typed routing settings and surfaces routing config diagnostics in JSON/stderr.
- Route trace JSON changed: `confidence` split into `selection_kind` + `match_evidence`; assessments use `match_evidence`.
- Launch-bundle routing/provenance renamed `route_confidence` to `match_evidence`, added top-level `selection_kind`, and bumped launch-bundle schema version to 2.
- Route acceptance policy centralized.
- Phase 1 model-first probes now gate OpenCode/Pi routing on harness-reported model slug lists (`opencode models`, `pi --list-models`) instead of provider-auth probe signals, with cache reuse keyed to model-list evidence and fail-closed availability when no matching slug exists.
- Phase 2/3 routing now threads explicit provider constraints from alias `provider` fields and `provider/model` CLI tokens, keeps bare-model provider inference as ordering-only (not a hard gate), enforces strict provider constraint matching (`openai` does not match `openai-codex`), and keeps provider-order ranking lenient for known variant suffixes.
- `mars models resolve` passthrough now fails cleanly when no harness-reported model slug matches under model-first routing, while still allowing direct constrained slugs (for example `openai/gpt-5.4-mini`) when a harness model list confirms them.
- Phase 4/5 routing now emits serializable `route_trace` assessments (candidate slugs, provider-filtered slugs, chosen slug/model, skip reasons) and threads that trace into launch-bundle routing JSON plus `mars models resolve` JSON/text output.
- Bare direct model tokens (no alias, no `provider/model` prefix) now route through unknown-provider candidate order (`pi, opencode, cursor`) instead of inferred provider-family order; explicit provider constraints remain strict and still fail closed when unsatisfied.

## [0.4.8-rc.7] - 2026-05-21

### Added
- Added `[agents.<name>]` launch-bundle overlay schema in `mars.toml` / `mars.local.toml` with routing + execution-policy fields and per-overlay `model-policies`.
- Added `[[settings.model-policies]]` project-level policy schema, shared with profile + overlay via one `ModelPolicyRule` type.
- Added launch-bundle provenance key `matched_policy_rule` (`overlay:<idx>`, `profile:<idx>`, `settings:<idx>`).

### Changed
- Launch-bundle policy resolution now composes model-policies across overlay → profile → settings (first match wins) and emits new provenance sources: `overlay`, `overlay-model-policy`, `settings-model-policy`.
- Launch-bundle model resolution now honors `[agents.<name>].model` before profile/default model.
- `mars.local.toml` overlay merge now does name-keyed replacement for `[agents.<name>]`; local `settings.model-policies` replaces base list.

## [0.4.8-rc.6] - 2026-05-21

### Changed
- Launch-bundle profile harness now pivots through candidate routing when profile harness missing on host; explicit CLI/alias harness still fails unavailable instead of silently pivoting.

## [0.4.8-rc.5] - 2026-05-20

### Added
- `[settings].default_model` support for launch-bundle model resolution (`cli > profile > project > error`), with `provenance.model_source = "project"` when selected.

## [0.4.8-rc.4] - 2026-05-20

### Added
- Bundle now exports agent_body alongside system_instruction for consumers needing decomposed prompt surfaces.

## [0.4.8-rc.3] - 2026-05-20

### Changed
- `mars build launch-bundle` now supports ad-hoc mode without `--agent`; requires `--model`, emits `agent: null`, and keeps profile tools/skills empty unless explicit `--skill` args are passed.
- Added canonical harness registry + host capability wrappers so harness names/targets/provider order/auth probe wiring come from one boundary.
- Added Pi probe/cache (`availability/pi.json`, 60s TTL, hidden `models __refresh-probe --target pi`) and wired routing to mark compatible Pi as `confirmed`, skip incompatible Pi, and keep passthrough when probe evidence is absent.
- Added live config boundaries (`config::targets`, `config::routing_settings`); legacy link migration now delegates to live target normalization.
- Removed `gemini` as a launch harness surface. Builtin `gemini` model alias stays valid.
- Model-alias `harness` values now validate against real launch harnesses.
- Model candidate ordering now falls back through `pi` → `opencode` → `cursor`; compatible Pi now reports runnable via Pi probe, incompatible Pi reports unavailable, and Cursor stays `Unknown` / `UniversalHarness` passthrough.
- Launch-bundle routing now emits `routing.route_confidence` plus provenance `route_confidence` / `candidates_tried`; execution policy schema now reserves optional `codex_rules` artifacts.
- Final routing gates now use provider/settings gate checks (not installed-only fallback), require auth evidence for native Codex/Claude provider matches, and fall through on stale/negative OpenCode cache evidence.
- Final resolver alignment: `mars models list`/`resolve` now share one evidence-aware harness resolver; invalid alias-harness writes fail fast; mixed-case harness names normalize; unknown/third-party models now fall back `opencode` before `cursor` when `pi` is absent.
- Shared routing engine (`src/routing/mod.rs`) now owns candidate evaluation, link filtering, auth gating, and the fallback ladder. Both `mars models` and `mars build launch-bundle` consume the same evaluator. ~470 lines of duplicate routing logic deleted.
- Launch-bundle routing + runnable resolution now consume one shared OpenCode probe snapshot (stale usable counts, stale negative does not), and `mars models resolve` passthrough now uses unknown-provider fallback candidates (`pi, opencode, cursor`) while keeping JSON provider `null`.

## [0.4.8-rc.2] - 2026-05-18

## [0.4.8-rc.1] - 2026-05-18

## [0.4.7] - 2026-05-17

### Changed
- Resolver semver selection now uses latest-compatible semantics by default: normal `mars sync` replays compatible locked versions for both direct and transitive deps, and falls back to newest compatible (not minimum) when no usable lock entry exists.
- Scoped `mars upgrade <name>` now maximizes only targeted sources while non-targeted deps keep lock-preferred latest-compatible behavior, preventing post-upgrade transitive downgrades on the next plain sync.

### Fixed
- `mars sync --frozen` now enforces lock-exact semver replay across the full graph: missing/incompatible/malformed locked versions and missing transitive lock entries now fail fast instead of silently re-resolving.
- Semver lock replay no longer depends on live tags: frozen mode now validates lock shape before any remote version listing and replays locked commits directly, so deleted tags do not break reproducible sync when the locked commit is still reachable.
- `release-on-main` tag push retries now verify expected tag commit before/after retries, confirm remote tag commit via `git ls-remote` (including annotated tag peel refs), fail fast on wrong-commit collisions, and only treat confirmed remote tags as success.

## [0.4.7-rc.1] - 2026-05-17

### Added
- `mars build launch-bundle` command. Builds a versioned launch bundle JSON from `.mars/` static state (`agents`, `skills`, `models-merged`) with routing/policy fields, prompt surface, tool metadata, provenance, and `scaffold_slots.* = "###SLOT###"` placeholders for Meridian-owned per-spawn content.

### Fixed
- Git test helpers no longer let inherited `GIT_*` repository environment redirect temp-repo commands into the caller checkout. Git subprocesses strip repo-scoped Git env before using explicit temp repo cwd.
- `mars build launch-bundle` harness routing now follows precedence correctly: `--harness` first; with `--model`, use model alias/provider/config/default (ignore profile harness); without `--model`, use profile/alias/provider/config/default. OpenAI aliases without explicit harness now route to `codex` instead of defaulting to `claude`, including auto-resolve aliases that miss model cache.
- `mars build launch-bundle` now emits runnable harness model routing (`routing.harness_model`, source, confidence) via deterministic resolver order (cached probe → provider-match → synthesized → passthrough), including warnings for unconfirmed/synthesized paths instead of silently launching non-runnable IDs.
- `mars build launch-bundle` now fails on invalid agent profile diagnostics (invalid field values, unknown harness names, non-overridable override fields) for both selected agent and inventory agents instead of shipping them as bundle warnings.
- Launch inventory in `mars build launch-bundle` now excludes `model-invocable: false` agents so hidden/internal agents do not leak into model-facing prompt context.
- `mars build launch-bundle` now resolves execution policy with full harness-override precedence (`CLI > matched harness override > profile > alias/default`) for `effort`, `approval`, `sandbox`, `autocompact`, and `autocompact_pct` instead of ignoring portable fields in `harness-overrides.<target>`.
- `mars build launch-bundle` prompt surface now uses the resolved harness-effective skill list (including `harness-overrides.<target>.skills` replacement semantics) instead of always using top-level `skills`.
- Cursor lowering now applies `harness-overrides.cursor` (not `harness-overrides.opencode`), and native lowering now uses effective harness-resolved `mcp-tools` for emission/lossiness checks.
- Cursor bundle warning/provenance now match contract text exactly and emit `provenance.harness_stability = "experimental"` only for Cursor targets.
- Native lowering now reports matched `harness-overrides.<target>.native-config` as `meridian-only` lossiness metadata (runtime-owned, not emitted into harness-native agent artifacts).
- `release-on-main` reruns now match prior releases by exact `Release-Trigger: <sha>` marker (not ancestry), can recreate a missing tag for an existing release commit, and still hand off the resolved tag to publish.
- `release-on-main` now fails if PR lookup API fails, selects one merged `main` PR deterministically (prefer exact `merge_commit_sha` match), and fails on ambiguous merged-PR candidates instead of unioning labels across all associated PRs.
- `release.yml` crate publish now ignores only already-published/already-uploaded `cargo publish` failures; all other publish errors fail the job with captured stderr.

### Changed
- Refactored launch-bundle policy resolver internals into focused private modules (`build::policy::{model,harness,execution,runnable}`) while preserving `resolve_policy` behavior and public API.
- Launch-bundle harness candidate selection now supports `settings.harness_order` global ordering (`mars.toml`): when set, it replaces provider defaults, selects the first installed recognized harness (`claude|codex|opencode|cursor|pi`), records `provenance.harness_source = "config-order"` (plus `harness_order_position`), and warns on invalid/empty/uninstalled order entries before falling through to `settings.default_harness` (when set) or hardcoded `claude`.
- Local full preflight skips git-mutating `mars version` release-flow tests. CI still runs the complete test suite.
- `mars build launch-bundle` prompt surface now follows Meridian static ordering: harness-aware skill variants, skill-type ordering/principle bookend, canonical report block, populated agent inventory, and model-override harness precedence (`--harness` > CLI model alias harness > provider/config/default).
- `mars build launch-bundle` now normalizes `tools.allowed`/`tools.disallowed` to target-harness canonical names (head-only; scoped payload preserved), leaves MCP tool names untouched, and warns on unknown tool names for first-class harnesses.
- Launch inventory lines now include `Fan-out: ...` metadata from fallback `model-policies` (`match.alias` / `match.model`, `no-fallback: false`) with exact-label dedupe. Alias-to-canonical-model dedupe is not yet wired in this slice.
- `mars build launch-bundle` now accepts `cursor` as a harness target, marks `provenance.harness_stability = "experimental"`, and emits an explicit experimental warning.
- Agent `harness-overrides.<harness>.native-config` now parses with shape-only validation and flows to launch bundle `execution_policy.native_config` (matching harness only, omitted when empty). Portable-key collisions warn but preserve values.
- Portable tool policy now supports `tools` map syntax (`allow`/`deny`) with mixed allow+deny preservation, consistent tool-name normalization, and harness-override replacement semantics for `tools`, `disallowed-tools`, and `mcp-tools` in launch bundles.
- Main-merge auto-release now defaults to RC for ambiguous `release:*` labels. Stable requires explicit `release:patch` or `release:stable`.
- Auto-release now computes RC versions as `vX.Y.Z-rc.N` from next stable patch base and writes PyPI version as PEP 440 `X.Y.ZrcN`.
- Publish workflow now marks RC GitHub releases as prerelease and publishes RC npm packages under dist-tag `rc` (stable stays `latest`).
- Release provenance now validates stable + RC commit subject, Cargo semver tag match, PyPI RC mapping (`vX.Y.Z-rc.N` -> `X.Y.ZrcN`), and npm package metadata version alignment (`version` + `optionalDependencies`).
- Release provenance now also verifies `CHANGELOG.md` has a semver release heading matching the tag version (`## [X.Y.Z] - ...` or `## [X.Y.Z-rc.N] - ...`).
- Auto-release tag push now uses `${{ github.token }}` for the `v*` push (while keeping main-branch push behavior) to avoid duplicate `release.yml` runs when checkout used `RELEASE_TOKEN`.

## [0.4.6] - 2026-05-16

### Changed
- Release publish workflow YAML fixed so tag/backfill release runs create jobs instead of failing at parse time.
- Release workflow: PR merges release only with a `release:*` label. CI creates the patch release commit and `vX.Y.Z` tag, then directly runs artifact publishing. Missing labels, `release:skip`, or direct `main` pushes skip auto-release. Tag pushes remain a manual/backfill publish path.
- `scripts/release.sh` deprecated. Stable releases are CI-owned.
- `scripts/manually-release.sh` added for emergency/backfill releases. Runs shared preflight, blocks empty `[Unreleased]`, updates version/changelog, commits, tags, and can push.
- Workflow docs: document label-gated auto-release and manual tag backfill behavior.
- `mars check`: parse and report malformed `model-policies` shape. Missing or empty `override` allowed; malformed `match`, `no-fallback`, or non-mapping `override` fails package check and `mars version`.
- Agent profile `autocompact_pct` spelling now matches `mars.toml` model aliases and Meridian generated artifacts.

## [0.4.4] - 2026-05-14

### Fixed
- Existing git mirror fetch now pulls tags (`git fetch --depth 1 --tags --prune-tags origin`). Previously the cached mirror update ran without `--tags`, so newly-pushed upstream tags were discoverable via `ls-remote` but missing from the local mirror — leading to `pathspec 'vX.Y.Z' did not match any file(s) known to git` on `mars sync` whenever a dependency cut a new release between syncs.

### Added
- Cargo-style transitive resolver semantics. Consumer lock now replayed only for direct deps; transitives resolve fresh from constraints each sync. `--frozen` still replays full graph. Pre-computed `direct_source_names` makes lock replay order-independent (direct dep first seen transitively still gets lock replay). 8 new EARS regression tests cover the selection policy.
- Canonical git URL identity. Shared `canonicalize_git_url` in `src/source/canonical.rs` normalizes SSH/HTTPS forms, trailing slashes, `.git` suffix, default port. Adopted in cache key generation, dirname generation, and `SourceId` construction so the same upstream converges to one cache entry/lock entry regardless of URL form. Includes SCP-vs-port detection for digit-leading path segments.
- Graph-backed `mars check`: re-resolves the dependency graph and treats missing skills as errors (not warnings). Same package-filter logic as real install — no false positives from auxiliary skills.
- `mars version --force`: bypass check errors and resolution failures for emergency releases.
- Fresh-context restart in resolver when a transitive constraint arrives after first resolution and would change the selected ref. Fixes order-dependent bug where `meridian-base` resolved to the MVS minimum (e.g. v0.3.0) under one intermediary's semver range, then ignored a later `Latest` from another intermediary because the package was already marked `Resolved`. Uses per-package ref-cycle oscillation detection (not a global pass cap) so large graphs with many independent late-arriving constraints converge cleanly. `latest_version` metadata propagates through the override path so sync upgrade reporting stays accurate. New tests: `restart_*`, `monotonic_restart_converges_for_more_than_32_packages`, `oscillating_ref_selection_errors_with_ref_cycle`, `restart_override_preserves_latest_version_metadata`.
- `managed_cmd()` helper — user-facing hints say `meridian mars <cmd>` when `MERIDIAN_MANAGED=1`, `mars <cmd>` otherwise. Replaced ~20 hardcoded `mars <cmd>` references across errors, warnings, and doctor output.
- OpenCode availability probe cache (60s TTL, stale-while-revalidate). `mars models list/resolve` no longer synchronously spawns `opencode providers list` + `opencode models` on every call — returns cached result and refreshes in the background. Eliminates ~2s per mars invocation after first probe.
- `mars unlink <target>` top-level subcommand. Removes a managed target directory and its settings entry. Owns its logic directly (not a shim over link).
- `cli::target` shared module for target-name normalization.

### Removed
- `mars link --unlink` flag. Use `mars unlink <target>` instead.

### Fixed
- Native harness targets no longer receive canonical `.mars/agents/*.md` copies. Prevents `.codex/agents` mixed `.md`/`.toml` output and invalid `.opencode/agents/*.md` canonical frontmatter/tools leakage.
- Codex agent lowering emits valid top-level TOML via serializer — fixes `unknown field agent` parse error and multiline TOML failures.
- Windows OpenCode probe cache cold path detects and runs `.bat`/`.cmd` shims, so fake or npm-installed `opencode` no longer skips cache population.
- `mars link --unlink` no longer auto-initializes a project in an empty directory before unlinking.
- `mars unlink` deletes the target directory before saving config, so a failed deletion doesn't leave settings mutated with the directory still on disk.

### Changed
- `upgrades_available` count in sync report now filters to direct deps only. Transitive dep upgrades no longer inflate the hint.
- Skill schema: replaced `invocation: explicit | implicit` enum with two independent booleans `model-invocable` and `user-invocable` (both default true). Per-harness lowering compiles each boolean to native fields: Claude gets both natively, Codex gets `allow_implicit_invocation` for model-invocable, Pi/Cursor get `disable-model-invocation`. Old fields (`invocation`, `disable-model-invocation`, `allow_implicit_invocation`) are hard errors.
- **BREAKING:** `autocompact` field on model aliases and agent profiles renamed from percentage (`u8`, 1–100) to token count (`u32`). New `autocompact_pct` field (`u8`, 1–100) carries the old percentage behavior. Both fields are meridian-only in native lowering. Downstream Meridian runtime owns precedence and harness conversion.

### Fixed
- `MERIDIAN_MANAGED=1` now suppresses agent artifacts in managed targets (`.claude/agents/`, `.opencode/agents/`). Previously target sync copied agents from `.mars/` to targets even under managed mode. Introduced `AgentSurfacePolicy` enum, unified three agent cleanup paths into `reconcile_native_agent_surfaces`, and fixed `mars link` to apply the same suppression policy as `mars sync`.
- Native harness cleanup dirs derived from `HarnessKind::all()` instead of a hardcoded list. Removed stale `.cursor` entry that isn't a `HarnessKind` variant.

## [0.2.5] - 2026-05-03

### Changed
- Suppressed `skill-field-dropped` and `agent-field-dropped` warnings for `Dropped` and `MeridianOnly` lossiness classifications. These are expected target-format gaps, not actionable.

## [0.2.4] - 2026-05-03

### Fixed
- Version drift false positives: `compatible_with_resolved` now handles `Latest` vs `Semver` constraints. When a concrete resolved version satisfies the semver constraint, reports `Compatible` instead of `PotentiallyConflicting`. Fixes ~20 spurious "potential version drift" warnings in diamond dependency trees.

## [0.2.3] - 2026-05-02

### Changed
- `.gitignore`: added `.claude/` and `.opencode/` generated artifacts.

## [0.2.2] - 2026-05-02

### Fixed
- Conventional flat-skill packages with root `SKILL.md` plus bootstrap docs now discover both skill and bootstrap docs.
- Native harness skill projection now runs inside `target_sync`, so projected skills stay expected during orphan cleanup and `mars link` can populate native skill dirs.
- Native skill projections now always refreshed on sync even when canonical is Skipped. Diverged projections repaired with warning.
- Windows TOML path escaping: `PathBuf` fields in config serialization now normalize backslashes to forward slashes. Prevents `\U` in `C:\Users\...` from being interpreted as TOML unicode escape sequences.

### Changed
- `ReaderIr` now embeds `ResolvedState` directly — eliminates decompose/reconstruct round-trip between reader and compiler stages. Removed dead `target_registry` field. Renamed `_sync_lock` → `sync_lock` in `LoadedConfig`. Removed redundant nested `dry_run` guard in `finalize()`.

### Added
- Bootstrap doc discovery. Package-level `bootstrap/<doc>/BOOTSTRAP.md` scanned during conventional discovery, flows through resolve/target/diff/apply pipeline, materializes to `.mars/bootstrap/`. Fallback/manifest discovery via `bootstrapDocs`/`bootstrap_docs` keys. `mars list` and `mars export` surface bootstrap docs.
- Skill variant projection. `variants/<harness>/<model>/SKILL.md` hierarchy discovered and validated. Native harness dirs get harness-selected variant; canonical `.mars/skills/<name>/variants/` preserved intact. `mars list` shows variant availability.
- Skill frontmatter compilation. Universal schema (`model-invocable`, `user-invocable`, `allowed-tools`, `license`, `metadata`) compiled to per-harness native fields via typed `SkillProfile` parser and lowering functions. Legacy `invocation`, `disable-model-invocation`, and `allow_implicit_invocation` now hard schema errors; migrate to `model-invocable` / `user-invocable`. Raw fallback for malformed frontmatter. `mars validate` checks skill schema.
- `sync::translate` module — `TranslatedOutput` type wraps `PlannedAction` with optional pre-translated content; `translate()` pass-through establishes insertion point for per-target format lowering.
- `TargetAdapter::write_config_entries` / `remove_config_entries` default-no-op methods + `ConfigEntry`/`ConfigEntryKind` placeholder types in `target/mod.rs`.
- Lock-driven orphan cleanup in `target_sync`: `cleanup_orphans` now iterates lock v2 `previous_managed_paths` directly instead of scanning hardcoded subdirectories (`agents/`, `skills/`, etc.).
- `mars version` CHANGELOG.md integration. Automatically promotes `[Unreleased]` → `[X.Y.Z] - YYYY-MM-DD`, inserts fresh empty `[Unreleased]`, stages alongside `mars.toml`. Warns when `[Unreleased]` section is empty. Silent skip when no CHANGELOG.md exists.
- `compiler::agents` — typed agent-profile schema parser: `AgentMode`, `HarnessKind`, `ApprovalMode`, `SandboxMode`, `EffortLevel`, `HarnessOverrides`, `ModelPolicyEntry`, `FanoutEntry`. `parse_agent_profile()` validates field values, flags legacy `models:`, rejects non-overridable fields in override blocks, collects `AgentDiagnostic` without aborting the sync.
- `compiler::agents::lower` — per-target agent lowering: `lower_to_claude()` (markdown+YAML per agent-compilation-mapping spec), `lower_to_codex()` (TOML `[agent]/[agent.config]/[agent.instructions]`), `lower_to_opencode()` (markdown+YAML), `lower_to_pi()` (simplified markdown). `harness-overrides` merged compile-time (D42). Lossiness classification `Exact/Approximate/Dropped/MeridianOnly` per field per target.
- Dual-surface compilation in `compiler::compile()` — `dual_surface_compile()` after `apply_plan()` writes `.mars/agents/`; scans harness-bound agents and writes native artifacts to `<project_root>/<harness_dir>/agents/<name>.<ext>`; emits lossiness warnings as diagnostics; non-fatal (D9). Universal agents (no `harness:`) produce only `.agents/` artifact.

## [0.1.19] - 2026-04-25

### Added
- Model availability classification. Each model now `runnable`, `unavailable`, or `unknown` based on installed harnesses and provider credentials.
- OpenCode provider probing. `opencode providers list` + `opencode models` detect available models through OpenCode harness.
- `--unavailable` flag. Show unavailable models in default list view.
- Availability fields in JSON output: `availability`, `availability_source`, `runnable_paths`.
- `probe_results.opencode` in JSON when OpenCode probing runs.
- **Three-step model resolve**: `mars models resolve` now tries alias → glob match against alias candidates → passthrough. Older versions work: `opus-4-6` → `claude-opus-4-6`. Unknown models pass through to harness with warning instead of erroring. Exit 0 always (cache is enrichment, not gate).
- **Three-tier `mars models list`**: default shows alias winners; `--all` shows all models matching any alias filter; `--catalog` dumps full models.dev cache.
- `auto_resolve_all()` — returns all alias filter candidates, not just winner. Used by `--all` listing and glob resolve.
- User-provided wildcards in resolve: `mars models resolve "*opus*"` uses pattern as-is; plain text auto-wraps as `*{input}*`.

### Fixed
- Offline mode no longer marks direct-harness models as unknown. Only OpenCode probing suppressed.
- Empty OpenCode provider list correctly classifies as unavailable, not unknown.
- OpenCode model slug matching requires exact match when model probe succeeds.
- Passthrough resolve works when cache unavailable (offline + first run). Cache load failure skips to passthrough instead of erroring.

### Changed
- Default `mars models list` prunes unavailable models. Use `--unavailable` to see them.
- `--all` expands alias candidates, does NOT show raw catalog. Use `--catalog` for that.
- `[settings.model_visibility]` now supports combined `include` + `exclude`.
- Visibility patterns match bare model ID, `provider/model`, or OpenCode slug based on slash count.
- `mars models resolve` includes availability annotation (never pruned).
- `--all` flag on `mars models list` redefined: was "show aliases with unavailable harnesses", now "show all alias-filter candidates across versions". No backwards compat needed.

## [0.1.16] - 2026-04-23

### Fixed
- Source name derivation splits on both `/` and `\` and strips drive prefixes — works cross-platform even when parsing Windows paths on Linux.
- Test assertions for Windows path source names expect last component, not full path.

## [0.1.15] - 2026-04-23

### Fixed
- Local path source name derivation uses `Path::file_name()` instead of string splitting — fixes `mars add`, `mars why`, `mars remove`, `mars override` on Windows.
- Archive cache temp path uses `Path::with_file_name()` instead of string concat.
- Content hash relative paths built from `Path::components()` instead of backslash replacement.

## [0.1.14] - 2026-04-23

### Changed
- `default_dest_path` / `parse_rename_dest` return `DestPath` directly, not `PathBuf`.
- `target_sync` uses `HashSet<String>` for cross-platform path comparison.
- `SourceSubpath` and `DestPath` share `normalize_relative_coordinate()` helper.
- `DestPath::item_name()` method added; `rsplit('/')` duplication removed.
- All `std::fs::canonicalize` replaced with `dunce::canonicalize` project-wide.
- Remaining `Command::new("git")` routed through `platform::process::run_git`.

### Fixed
- Windows 8.3 short-name path mismatches in `find_root` and `merge_override` tests.

## [0.1.13] - 2026-04-23

### Changed
- `DestPath` refactored from `PathBuf`-backed to `String`-backed normalized forward-slash coordinate. Lock keys and map keys now consistent across platforms. `resolve(root)` is the only path to native filesystem paths.
- `default_dest_path` and `parse_rename_dest` return `DestPath` directly, not `PathBuf`.
- `target_sync` uses `HashSet<String>` for cross-platform path comparison.
- `SourceSubpath` and `DestPath` share internal `normalize_relative_coordinate()` helper.
- Added `DestPath::item_name()` method; deduplicated `rsplit('/')` pattern.
- All `std::fs::canonicalize` replaced with `dunce::canonicalize` project-wide.
- Remaining `Command::new("git")` in `version.rs` and `merge/mod.rs` routed through `platform::process::run_git`.

### Fixed
- Windows lock files with backslash paths now normalize to forward slashes on load.
- `mars rename` validates destination path before storing mutation.
- Invalid rename destinations in config return error instead of panic.
- `mars adopt` handles invalid target-relative paths gracefully.
- Cache base filename uses underscore instead of colon for Windows compatibility.
- Doctor target divergence warnings use forward-slash display paths.
- MarsContext canonicalization uses `dunce` to avoid `\\?\` prefix on Windows.
- Rename destination normalization handles backslash paths.
- Path source name derivation uses forward-slash-only splitting for cross-platform consistency.

## [0.1.10] - 2026-04-23

### Fixed
- Windows test build no longer compiles POSIX-only symlink fixtures.

## [0.1.9] - 2026-04-23

### Added
- Windows CI job for `cargo fmt --all --check` and `cargo test -q`.
- Windows release artifacts: `mars-windows-x64.exe` binary and PyPI wheel.
- Windows npm package: `@meridian-flow/mars-agents-win32-x64`.
- Windows PowerShell smoke testing guide (`docs/smoke-testing-windows.md`).
- `crate::platform` boundary module for cross-platform operations.

### Changed
- Cache root default now uses OS cache directories (`dirs::cache_dir()`).
- Cache component names use hash suffix for collision prevention.
- Directory replacement uses explicit `replace_generated_dir` with rollback.
- Cache finalization uses `publish_cache_dir_if_absent` for race handling.
- Git invocation centralized in `platform::process::run_git`.
- Source path classification centralized in `platform::path_syntax`.
- POSIX smoke guide renamed to `docs/smoke-testing-posix.md` with platform note.
- `docs/commands.md`: `mars link` described as copy, not symlink.

### Fixed
- Explicit-port URLs (e.g., `git://host:19424/repo.git`) no longer produce cache directories with colons.
- Windows-invalid characters in cache component names are sanitized.
- Windows reserved device names in cache paths are escaped.
- Filesystem errors now include operation name and path in diagnostics.

## [0.1.8] - 2026-04-19

### Added
- `mars version` runs package check before versioning — catches invalid frontmatter and missing SKILL.md before tagging.

## [0.1.7] - 2026-04-19

### Fixed
- `local-shadow` warning suppressed when content checksums match — no noise from diamond dependencies pulling same skill from multiple paths.

## [0.1.6] - 2026-04-19

### Changed
- `ManifestDep` unified for URL and path deps — eliminated `collect_path_manifest_requests` special case.
- Removed dead `ResolvedGraph.id_index` field (internal `ResolverContext.id_index` kept for duplicate detection).

### Fixed
- Filtered deps now resolve version without materializing transitive items.
- `Latest` constraint validation no longer bypassed.
- Constraint syntax comparison uses semver semantics, not string equality.
- Skill lookup checks same package first, then all resolved packages.

### Internal
- Resolver god module (4.4k lines) split into 10 focused modules.
- `ResolverContext` tracks version constraints and materialization filters separately.

## [0.1.4] - 2026-04-18

### Added
- `mars add` auto-inits a missing project at `--root` or cwd before adding a source.
- `mars link` auto-inits a missing project before managing a target directory.
- Smoke coverage for bootstrap and root-discovery flows.

### Changed
- `mars add` and context commands walk up to filesystem root, not git root.
- Walk-up boundary is now filesystem root on all platforms (Unix `/`, Windows `C:\`, UNC paths).
- `mars init` creates project at cwd (or `--root` target) without walking up.
- Auto-init applies to `mars add` and `mars link`; `mars sync` still errors on a missing project.
- `--root` for context commands sets walk-up start path, not direct project target.
- Error message now says "filesystem root" instead of "repository root".
- Windows compatibility documented as first-class invariant in AGENTS.md.

## [0.1.3] - 2026-04-16

### Added
- `mars adopt` moves unmanaged target items into `.mars-src/`, then syncs.
- `.mars-src` is now project-local source for agents and skills.
- Non-package repos can mirror local items across `.agents`, `.claude`, and other targets.
- Smoke coverage and docs for adopt/local source flow.

### Changed
- Sync now reads `.mars-src` local items even without `[package]`.
- Legacy repo-root `agents/` and `skills/` stay supported only for package repos.
- `.mars-src` wins if both local roots define the same item.

### Fixed
- `mars list` now shows adopted/local `.mars-src` items after sync.

## [0.1.2] - 2026-04-16

### Added
- Subpath support. One repo can hold many packages.
- Parser understands more source forms: GitHub, GitLab, generic git, local path.
- Smoke testing guide added.
- Repo now uses `meridian-dev-workflow` through Mars.

### Changed
- Fallback discovery now does explicit paths first, then nearest non-empty layer.
- Same-layer fallback picks first deterministic match.
- `mars add` supports `--subpath`.
- Docs now explain subpath and supported source forms.

### Fixed
- `meridian-dev-workflow` install no longer breaks on mirrored `caveman` layout.
- GitLab-like URLs keep explicit ports.
- Parser clippy failure fixed for release checks.
