# QA Test Strategy: Mars Launch-Bundle Follow-up Audit

## Scope

Audit target: Mars-side follow-up slice for `mars build launch-bundle` and native materialization/lowering. Source of truth: the passed requirements, `design2/` bundle/schema/tool-policy/projection docs from the Meridian design package, explorer report `p942`, and the current worktree implementation/tests.

Shipped slice under audit:

- Cursor as an experimental launch-bundle target.
- `native-config` schema parsing and launch-bundle output.
- Portable tool policy mixed allow/deny preservation.
- Harness override precedence for launch-bundle execution policy, tools, and skills.
- Native lowering consistency for Cursor, `mcp-tools`, and `native-config` lossiness.

Out of scope for this QA pass:

- Model-policy resolution drift — tracked in GitHub issue #43.
- Inventory fanout drift — tracked in GitHub issue #44.
- Meridian consumer projection tests for Claude/Codex/OpenCode/Cursor runtime argv/env/settings. Those belong in `meridian-cli`, not this Mars audit.

## Reviewer incorporation and execution update

Reviewer `p944` requested two changes before execution:

- Cover the harder resolved-harness path, not only an explicit
  `--harness cursor` path. The executed launch-bundle test therefore uses a
  CLI model alias whose config resolves to Cursor, with profile/root/OpenCode
  decoys, and asserts that skills, tools, and `native_config` all follow the
  resolved Cursor harness.
- Extend prompt-isolation assertions beyond `system_instruction`. The executed
  helper checks `system_instruction`, `inventory_prompt`, and every
  `supplemental_documents[*].content`.

The sync Cursor test was also narrowed to target wiring plus observable
lossiness behavior. It does not claim static materialization consumes
`native-config`; this slice intentionally keeps native config Meridian-runtime
only.

## Current test-suite judgment

`tests/launch_bundle.rs` is still the correct primary tier for launch-bundle behavior. It exercises the public CLI, synced `.mars/` state, profile parsing, model/config loading, skill loading, bundle JSON serialization, and warning aggregation through the same boundary Meridian will call.

The previous blocker recommendations have mostly been executed in-place rather than by splitting the file. That is acceptable for this follow-up: splitting the 1498-line file is structural cleanup, not a blocker-confidence move. Do not spend this pass on a multi-file split unless the implementation work already touches enough tests to make the split cheaper than preserving the current file.

The remaining gaps are narrow and slice-specific. Add tests only where they protect target selection or data preservation that could regress silently.

## Tier audit

| File / area | Current tier | Correct tier | Directive |
|---|---:|---:|---|
| `tests/launch_bundle.rs` | CLI integration | CLI integration | Keep as the launch-bundle contract boundary. Add one Cursor-specific override test here. Do not replace these tests with private function tests. |
| `tests/sync_behavior.rs` | CLI integration | CLI integration | Add one `.cursor` native materialization test here because target wiring and emitted file shape are sync-level behavior. |
| `tests/common/mod.rs` | Shared integration fixture | Shared integration fixture | Leave as-is for this pass. Do not move `setup_bundle_project*` unless doing the deferred split. |
| `src/compiler/agents/mod.rs` unit tests | Unit | Unit | Add only pure parser-shape coverage for `native-config` arrays/null handling. This is cheaper and more precise than another full CLI fixture. |
| `src/compiler/agents/lower.rs` unit tests | Unit | Unit | Keep existing Cursor/Codex lossiness tests. Do not add more lowerer unit tests unless the sync-level Cursor target test fails to isolate the issue. |
| `tests/model_config.rs`, `tests/models_*` | CLI integration | CLI integration | Leave untouched. Launch-bundle tests already cover the model-policy inputs needed for this slice; model-policy follow-ups are issue #43. |

## Coverage map

| Interface / invariant | Covered now | Remaining gap | Directive |
|---|---|---|---|
| Bundle schema v1 fields and scaffold placeholders | `build_launch_bundle_outputs_schema_and_slot_placeholders` | Good | Keep. Do not assert exact pretty-print whitespace. |
| Mars rejects Meridian-owned per-spawn prompt input | `build_launch_bundle_rejects_prompt_file_flag` | Good | Keep. This protects the ownership boundary even though Clap performs the rejection. |
| Cursor launch-bundle target accepted from CLI | `build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental` | Good | Keep. |
| Cursor launch-bundle target accepted from profile | `build_launch_bundle_accepts_profile_cursor_harness` | Good | Keep. |
| Cursor experimental warning/provenance | Same two Cursor tests | Good for routing only | In the new Cursor override test, assert the same warning/provenance only if it is already convenient; do not duplicate a standalone warning test. |
| Cursor target-specific launch-bundle overrides | Not covered at launch-bundle boundary. Existing `cursor_lowering_uses_cursor_override_not_opencode_override` covers native lowering only. | High-value gap: a Cursor bundle could accidentally use OpenCode/root overrides while simple Cursor routing still passes. | Added `build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy` in `tests/launch_bundle.rs`. The fixture uses a CLI model alias resolving to Cursor so the test covers the harder resolved-harness path. It asserts `skills_metadata.loaded`, prompt skill content, `tools.allowed`, `tools.disallowed`, `tools.mcp`, `execution_policy.native_config`, `provenance.native_config_source`, and absence of decoy/native config strings from the full prompt surface. |
| `native-config` emitted for matched harness and kept out of prompt | `build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean` | Good for Codex scalar/map and prompt isolation | Keep. The new Cursor override test should additionally prove non-matching harness native config is not selected. |
| `native-config` invalid top-level shape fails through CLI | `build_launch_bundle_invalid_native_config_shape_fails_with_diagnostic`; parser unit `harness_override_native_config_invalid_shape_produces_diagnostic` | Good for non-map top-level | Keep. |
| `native-config` accepted value shapes | Parser unit `harness_override_native_config_parses_shape_only` covers bool and nested map | Missing array preservation and nested null rejection from the explicit schema | Add parser unit `harness_override_native_config_accepts_arrays_and_rejects_null_values`: first fixture asserts arrays containing scalar values survive as JSON arrays; second fixture with `native-config.some_key: null` emits `InvalidFieldValue` for `harness-overrides.<harness>.native-config.some_key` and omits the native config map. |
| Portable tool policy mixed allow/deny at bundle root and override | `build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement`; parser unit `effective_tool_policy_uses_harness_override_replacements` | Good | Keep. Do not add one test per harness; Mars emits the resolved bundle, Meridian projects per harness. |
| Harness override precedence for execution policy | `build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias`; `build_launch_bundle_cli_overrides_profile_execution_policy_fields`; `build_launch_bundle_profile_execution_policy_flows_without_cli_override` | Good for this slice | Keep. Do not add model-policy override cases in this pass; issue #43 owns that. |
| Harness override replacement for skills | `build_launch_bundle_uses_harness_override_skills_for_prompt_surface`; extra skill merge test | Covered for Codex, not Cursor | New Cursor override test covers Cursor-specific branch. |
| Harness override replacement for tools/MCP | `build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement` | Covered for Codex, not Cursor | New Cursor override test covers Cursor-specific branch. |
| Cursor native materialization target wiring | `lower_for_harness_dispatches_correctly` checks lowerer dispatch; no sync-level `.cursor` target fixture | High-value gap: sync could fail to write `.cursor/agents/<name>.md` or cross-wire target paths while lowerer unit tests pass. | Added `sync_cursor_native_agent_target_emits_cursor_markdown_and_lossiness_warning` in `tests/sync_behavior.rs`. It configures `targets = [".cursor"]`, `agent_emission = "always"`, and an agent with Cursor-only `mcp-tools`; then asserts `.cursor/agents/<name>.md` exists, non-Cursor native artifacts do not, emitted Markdown has name/model/body, emitted Markdown omits `mcp-tools`/`native-config`, and stderr contains behavior-level Cursor `mcp-tools` lossiness evidence. |
| Cursor lowerer chooses Cursor override, not OpenCode override | `cursor_lowering_uses_cursor_override_not_opencode_override` | Good at unit tier | Keep. The sync test above guards the public target wiring. |
| Codex native lowering reports native-config as Meridian-only | `codex_native_config_lossiness_uses_matching_override` | Good | Keep. Do not require user-facing warnings for Meridian-only fields; current compiler intentionally suppresses dropped/Meridian-only warnings and warns only for approximate fields. |
| Static native materialization of `native-config` | Docs state current native lowering treats `native-config` as Meridian runtime-only | No test should expect static native artifacts to contain native config in this slice | Add no tests that require `.claude/settings.json`, `opencode.json`, or Cursor rule files from Mars. That is future/native-materialization work. |

## Keep rationale

Keep these tests because they protect real behavior at the correct boundary:

- `build_launch_bundle_outputs_schema_and_slot_placeholders` — bundle shape and Meridian-owned scaffold slots.
- `build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental` and `build_launch_bundle_accepts_profile_cursor_harness` — Cursor routing and experimental contract.
- `build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean` — highest-risk prompt isolation invariant.
- `build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement` — guards the c859 class of allow/deny loss.
- `build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias` — protects the slice's override precedence rule.
- `build_launch_bundle_has_canonical_prompt_surface_for_small_fixture` — keep one compact exact prompt grammar guard. Do not add more full prompt snapshots.
- `cursor_lowering_uses_cursor_override_not_opencode_override` — the narrow unit guard for Cursor-vs-OpenCode lowering drift.
- Existing parser units for `native-config` and tool policy — pure parser contracts are cheaper and clearer at unit tier.

## Delete manifest

No immediate deletions for this follow-up pass.

Previous low-value assertions have already been removed or rewritten: the pretty-JSON whitespace assertion is gone, and the principle bookend check now asserts before/after `# Report` rather than a raw body count. The remaining negative parser cases in `tests/launch_bundle.rs` duplicate some unit parser coverage, but they also prove fatal profile diagnostics abort the public launch-bundle command with actionable stderr. Do not delete them in this pass.

Deferred cleanup only:

| Target | Action when cleanup is approved | Why deferred |
|---|---|---|
| `tests/launch_bundle.rs` | Split into schema/policy/prompt/inventory/errors files and move local fixture builders to common support. | Valuable for reviewability, but not needed to close the follow-up behavioral gaps. |
| Additional exact prompt snapshots | Do not add. Keep only the current compact fixture. | More snapshots would pin formatting beyond the model-facing contract. |

## Tests to add now

1. `build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy`
   - File: `tests/launch_bundle.rs`.
   - Fixture:
     - Profile model set.
     - Root `skills`, `tools`, `mcp-tools` with decoy values.
     - `harness-overrides.opencode` with decoy skills/tools/native-config.
     - `harness-overrides.cursor` with expected skills/tools/native-config.
     - `[models.cursoralias].harness = "cursor"`.
   - Command: `mars build launch-bundle --agent reviewer --model cursoralias`.
   - Assert:
     - `routing.harness == "cursor"`.
     - `provenance.harness_stability == "experimental"`.
     - Cursor skill is loaded and root/OpenCode skill content is absent.
     - `tools.allowed`, `tools.disallowed`, and `tools.mcp` equal the Cursor override values.
     - `execution_policy.native_config` equals the Cursor override map and excludes OpenCode decoy values.
     - Native-config keys/values do not appear in any prompt surface text (`system_instruction`, `inventory_prompt`, supplemental document content).

2. `harness_override_native_config_accepts_arrays_and_rejects_null_values`
   - File: unit tests in `src/compiler/agents/mod.rs`.
   - First fixture: `native-config` contains an array value and a nested map; assert parsed JSON values preserve both.
   - Second fixture: `native-config` contains a null value; assert an `InvalidFieldValue` diagnostic names the nested key and the invalid native-config is not available for that harness.

3. `sync_cursor_native_agent_target_emits_cursor_markdown_and_lossiness_warning`
   - File: `tests/sync_behavior.rs`.
   - Fixture: project settings `targets = [".cursor"]`, `agent_emission = "always"`; agent has `harness: cursor`, Cursor override, and OpenCode decoy override.
   - Command: `mars sync --root <project>`.
   - Assert:
     - `.mars/agents/<name>.md` exists.
     - `.cursor/agents/<name>.md` exists.
     - `.opencode/agents/<name>.md` and `.codex/agents/<name>.toml` do not exist.
     - Cursor Markdown includes expected name/model/body.
     - Cursor Markdown does not contain `tools:`, `mcp-tools:`, `native-config`, or decoy override values.
     - stderr contains behavior-level evidence for Cursor `mcp-tools` lossiness without pinning exact prose.

## Explicit no-op / defer rationale

- Do not add model-policy resolution tests here. That work is tracked by issue #43 and was explicitly adjudicated out of this QA pass.
- Do not add inventory fanout tests here. That work is tracked by issue #44 and was explicitly adjudicated out of this QA pass.
- Do not add Meridian harness projection tests in Mars. Mars only emits bundle data and native artifacts; Meridian owns runtime argv/env/settings projection.
- Do not add Codex/Claude/OpenCode native-config projection tests to Mars static materialization. Current Mars native lowering records `native-config` as Meridian runtime-only/lossy; static projection is not implemented in this slice.
- Do not add Gemini tests. Gemini is not a current Mars/Meridian target.
- Do not add Pi launch-bundle tests beyond existing enum/fallback behavior. Pi is a future first-class contract; this slice's target addition is Cursor.
- Do not add broad parser tests for every native-config scalar type. The array/null unit plus existing boolean/nested-map coverage is enough for the shape contract.
- Do not split `tests/launch_bundle.rs` as part of the behavioral follow-up. Record it as cleanup, not a blocker.

## Parallel-safe implementation phases

### Phase A — Cursor launch-bundle override boundary

Ownership: `tests/launch_bundle.rs` only.

Actions:
- Add `build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy`.
- Reuse existing `setup_bundle_project` helpers.
- Do not modify production code unless the new test exposes a real bug.

Completion criterion:

```bash
cargo test --test launch_bundle build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy
```

### Phase B — Native-config parser shape edge

Ownership: `src/compiler/agents/mod.rs` test module only.

Actions:
- Add array-preservation and null-rejection parser coverage.
- Keep assertions on public parser outputs/diagnostics, not private helper functions.

Completion criterion:

```bash
cargo test harness_override_native_config
```

### Phase C — Cursor native materialization wiring

Ownership: `tests/sync_behavior.rs` only.

Actions:
- Add `sync_cursor_native_agent_target_emits_cursor_markdown_and_lossiness_warning`.
- Assert output files and lossiness warning through the `mars sync` CLI boundary.

Completion criterion:

```bash
cargo test --test sync_behavior sync_cursor_native_agent_target_emits_cursor_markdown_and_lossiness_warning
```

### Phase D — Follow-up gate

Ownership: no new test files unless fixes from failed tests require them.

Completion criterion:

```bash
cargo test --test launch_bundle --test sync_behavior
cargo test harness_override_native_config
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

## Validation manifest

After the strategy is executed and passing, qa-lead can mark these files `# qa-validated` for this follow-up slice:

- `src/cli/build.rs` — `--harness cursor`, launch-bundle CLI argument surface, and no prompt-file ownership leak.
- `src/build/bundle.rs` — `execution_policy.native_config`, `tools`, scaffold slots, schema v1 serialization.
- `src/build/mod.rs` — agent load/parse failure handling, prompt/policy/tool assembly, warning aggregation.
- `src/build/policy.rs` — Cursor stability warning, harness resolution, matched harness override, native-config provenance, execution-policy precedence.
- `src/build/prompt.rs` — harness-specific skill selection and prompt-surface isolation from native-config.
- `src/compiler/agents/mod.rs` — Cursor harness enum, `native-config` shape validation, mixed tool policy parsing, harness override effective values.
- `src/compiler/agents/lower.rs` — Cursor lowering dispatch and Cursor/OpenCode override separation; native-config/mcp lossiness.
- `src/compiler/mod.rs` — sync-level native target materialization for `.cursor`.
- `tests/launch_bundle.rs` — launch-bundle CLI contract coverage.
- `tests/sync_behavior.rs` — sync/native materialization coverage.

## Validation commands for qa-lead

```bash
cargo test --test launch_bundle
cargo test --test sync_behavior
cargo test harness_override_native_config
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```
