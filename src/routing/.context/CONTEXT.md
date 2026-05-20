# src/routing/ — canonical candidate evaluator

## Contracts

- **`evaluate_candidates()` is the ONLY candidate evaluator.** Both `mars models` and `mars build` call only this function. There must be no second candidate evaluator anywhere in the codebase. Parity is an invariant, not a coincidence.
- **`evaluate_fixed_harness()` evaluates one fixed harness without fallback.** Used by CLI/profile/alias fixed-selection paths. Returns a `CandidateAssessment` directly — no candidate iteration, no fallback chain.
- **RouteConfidence semantics:**
  - `Explicit`: fixed selection from CLI/profile/alias (v1 compat; maps to confirmed in practice)
  - `Confirmed`: native provider match + authenticated (Claude/Codex) OR compatible Pi probe
  - `Likely`: OpenCode cached provider+model evidence
  - `Passthrough`: universal (Cursor), Pi without probe, OpenCode unknown-provider
- **`RouteSource` (WHO chose the route) and `RouteConfidence` (HOW strong is the evidence) are separate dimensions.** Do not conflate them.
- **Link filtering: ONLY `KnownHarness` links filter candidates.** Generic targets (`.agents`, `agents`, unknown names) are invisible to routing. See [config/targets.rs](../config/targets.rs) for link normalization.
- **When link constraints block all candidates and all fallbacks, the first linked harness is selected with `Passthrough` confidence** — no unrelated harness leaks through.

## Architecture

**Evaluation order:**
1. Build candidate list from `settings_harness_order` (if set) or `provider_candidate_order` (via `harness::registry`)
2. Filter candidates by `linked_harnesses` (only `KnownHarness` links; generic targets do NOT filter)
3. For each candidate: installed check → native match + auth → OpenCode probe → Pi probe → Cursor passthrough
4. Fallback chain: config `default_harness` → first linked harness → hardcoded `claude`
5. Link constraints prevent config-default and hardcoded fallbacks from routing outside known links

**Key input struct** `RoutingInput`: carries model_id, provider, harness order config, installed set, linked harnesses, OpenCode probe, Pi probe.

**Output struct** `RoutingTrace`: selected harness, confidence, source, candidates_tried, per-candidate assessments, diagnostics.

## Rationale

- **Single evaluator prevents drift.** Before PR #51, `mars models` and `mars build` could route differently for the same inputs. Now both call the same `evaluate_candidates()`.
- **Link constraints block unreachable fallbacks.** Without link filtering, `config.default_harness = "codex"` would route to Codex even when the project only links `.claude`. This was a bug — linked harnesses express project intent, and routing outside them must not happen silently.

## Patterns

- **Auth injection for testing:** Use `evaluate_candidates_with_auth(input, always_authed)` or `evaluate_candidates_with_auth(input, never_authed)` to control auth without real subprocess calls.
- **Probe injection for testing:** Pass `Some(&probe_result)` or `None` via `RoutingInput` to control OpenCode/Pi probe behavior.
- **Parity testing:** Same inputs to `mars models resolve --json` and `mars build launch-bundle --json` must yield the same `harness` and `route_confidence` fields.
