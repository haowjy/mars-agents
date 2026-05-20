# src/routing/

Single-file module: `mod.rs`. The canonical candidate evaluator for harness selection.

## Contracts

### Single evaluator invariant

`evaluate_candidates()` is the **only** candidate evaluator in the codebase.
Both `mars models` (resolve/list) and `mars build launch-bundle` call it —
this is what makes their routing outputs consistent. A second evaluator anywhere
would break the parity invariant.

`evaluate_fixed_harness()` evaluates one specific harness without fallback.
Used when the caller has already committed to a fixed harness choice
(CLI `--harness`, profile `harness:`, alias `harness:`). It returns a single
`CandidateAssessment` — the caller decides what to do with a failed fixed selection.

### `RoutingInput` fields

| Field | Role |
|---|---|
| `model_id` | Resolved model identifier (used for OpenCode slug matching) |
| `provider` | Optional provider name (determines native affinity and candidate order) |
| `settings_harness_order` | Raw `harness_order` from config, if set |
| `config_default_harness` | Raw `default_harness` from config, if set |
| `installed_harnesses` | Set of harness names found on PATH |
| `linked_harnesses` | Known harness names from `config::targets` link normalization |
| `opencode_probe_result` | Cached OpenCode probe (provider/model evidence) |
| `pi_probe_result` | Cached Pi probe (binary + help-surface compatibility) |

### `RouteConfidence` semantics

| Value | Evidence |
|---|---|
| `Explicit` | Fixed selection from CLI/profile/alias (v1 bundle compat) |
| `Confirmed` | Native provider match + authenticated (Claude/Codex) OR compatible Pi probe |
| `Likely` | OpenCode cached provider+model evidence |
| `Passthrough` | Universal passthrough (Cursor), Pi without fresh probe, OpenCode unknown-provider, config-default fallback |

`RouteSource` records **who** chose the route. `RouteConfidence` records **how strong** the evidence is.
These are separate dimensions — a `ConfigDefault` source is always `Passthrough` confidence;
a `Provider` source can be `Confirmed` or `Passthrough` depending on evidence.

### Link filtering rule

Only `KnownHarness` links (from `config::targets::normalize_link`) filter routing candidates.
Generic targets (`.agents`, `agents`, unknown names) and path-like targets are **invisible**
to routing — they are materialization-only. See `config::targets` for normalization details.

When known linked harnesses exist:
- Auto-routing candidates are filtered to the linked set before evaluation
- `settings.default_harness` outside the linked set is ignored (with diagnostic)
- Hardcoded `claude` fallback is blocked (linked harnesses select themselves instead)

## Architecture

```
RoutingInput
    │
    ├─ settings_harness_order? → parse + link-filter → ConfigOrder candidates
    ├─ (no order) → provider_candidate_order → link-filter → Provider candidates
    │
    └─ for each candidate:
           not installed        → skip (not_installed)
           native provider + auth  → Confirmed ✓
           opencode + known provider + probe → Likely ✓
           opencode + unknown provider → Passthrough ✓
           pi + compatible probe → Confirmed ✓
           pi + no probe        → Passthrough ✓
           cursor               → Passthrough ✓
           else                 → skip

    exhausted candidates → config_default_harness → linked fallback → hardcoded claude
                           (link constraints can block each of these)
```

## Rationale

Single evaluator prevents `mars models` and `mars build` from drifting on
routing decisions. Before this module, both had independent candidate evaluation
logic that could diverge on harness ordering, auth gates, and probe handling.

Link constraints blocking hardcoded/config-default fallback is intentional:
`settings.targets = [".opencode"]` signals project intent to use OpenCode.
Silently routing to Claude as a fallback contradicts that intent.

Pi upgrade from Passthrough→Confirmed: before PR #51, a Pi binary on PATH was
always Passthrough (unknown capability). With the Pi probe, Mars knows whether
the installed Pi supports the required spawn flags, so it can express Confirmed
confidence.

Route facts (`Passthrough` confidence, `provider-match` source, `unknown`
harness_model_confidence) are **not warnings**. They belong in routing/provenance
fields. Warnings are for unexpected user-actionable degraded states — e.g., "linked
harness constraints left no eligible candidates." The distinction is enforced by
`build/policy/runnable.rs::resolve_routing()` returning `warnings: Vec::new()` always;
the caller layer owns warning promotion.

## Patterns

**Test without real auth probes:**

```rust
let trace = evaluate_candidates_with_auth(&input, |_harness| true /* always_authed */);
```

**Simulate Pi compatibility:**

```rust
let pi_probe = PiProbeResult { compatible: true, ..PiProbeResult::default() };
// pass Some(&pi_probe) as pi_probe_result in RoutingInput
```

**Parity smoke test** (run in a temp project with known config):

```bash
HARNESS=$(mars models resolve gpt-5.4-mini --json | jq -r '.harness')
BUNDLE_HARNESS=$(mars build launch-bundle --model gpt-5.4-mini --json | jq -r '.routing.harness')
[ "$HARNESS" = "$BUNDLE_HARNESS" ] || echo "DRIFT"
```
