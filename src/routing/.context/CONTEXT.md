# src/routing/

Multi-file module: `mod.rs` (evaluator), `slug.rs` (slug matching), `acceptance.rs` (policy), `report.rs` (serialization). The canonical candidate evaluator for harness selection.

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

**The evaluator never errors** — `evaluate_candidates()` always returns a `RoutingTrace`.
Acceptance decisions belong to callers via `accept_route()` / `accept_assessment()`.

### `RoutingInput` fields

| Field | Role |
|---|---|
| `model_id` | Resolved model identifier (used for OpenCode/Pi slug matching) |
| `provider_for_order` | Optional provider name (determines native affinity and candidate order) |
| `provider_constraint` | Optional provider constraint (filters slug selection, excludes mismatched native harnesses) |
| `settings_provider_order` | Raw `provider_order` from config, if set |
| `settings_harness_order` | Raw `harness_order` from config, if set |
| `config_default_harness` | Raw `default_harness` from config, if set |
| `installed_harnesses` | Set of harness names found on PATH |
| `linked_harnesses` | Known harness names from `config::targets` link normalization |
| `opencode_probe_result` | Cached OpenCode probe (provider/model evidence) |
| `pi_probe_result` | Cached Pi probe (binary + help-surface compatibility) |

### `SelectionKind` semantics

| Value | Meaning |
|---|---|
| `Auto` | Selected by candidate evaluation loop (first acceptable harness) |
| `Fixed` | Caller committed to a specific harness (CLI/profile/alias) |
| `ConfigDefault` | Fell through to `settings.default_harness` |
| `LinkedFallback` | No eligible candidates; linked harnesses selected themselves |
| `HardcodedDefault` | Nothing else matched; defaulting to `pi` |

### `MatchEvidence` semantics

| Value | Evidence |
|---|---|
| `Confirmed` | Native provider match + authenticated, OR compatible Pi probe, OR positive OpenCode probe |
| `Constrained` | Same as Confirmed, but a `provider_constraint` was active |
| `Passthrough` | Universal harness (Cursor), Pi without fresh probe, OpenCode unknown-provider, config-default fallback |
| `None` | No evidence — candidate was rejected |

`RouteSource` records **who** chose the route. `SelectionKind` records **how** the harness was selected. `MatchEvidence` records **what slug evidence exists**. These are orthogonal dimensions — a `ConfigDefault` source is always `ConfigDefault` kind with `Passthrough` evidence; a `Provider` source can be `Auto` kind with `Confirmed`, `Constrained`, or `Passthrough` evidence depending on what matched.

### `slug.rs` contracts

**`SlugParts<'a>` borrows its input** — avoids allocation in hot slug-scanning loops.
Callers needing owned data use `SlugMatch` or `.to_string()`.

- `parse(slug)` — splits on first `/`; provider is everything before, model_id everything after (may contain nested `/`). Returns `None` for empty provider or empty model_id.
- `find_model_matches(model_id, slugs)` — returns all slugs whose model_id matches (case-insensitive, dot-dash normalized).
- `find_exact_match(model_id, provider, slugs)` — returns first slug matching both provider and model_id, preferring exact provider match over variant (e.g. `openai` over `openai-codex`).

### `acceptance.rs` contracts

**`MatchPolicy` controls strictness:**

| Policy | Accepts |
|---|---|
| `RequireSlugEvidence` | `Confirmed` or `Constrained` only |
| `AllowPassthrough` | `Confirmed`, `Constrained`, or `Passthrough` |
| `InstalledOnly` | Any evidence (or none), as long as harness is installed |

**`accept_route()` vs `accept_assessment()`:**
- `accept_route(trace, installed, policy)` — validates a full `RoutingTrace` against a policy. Used by callers who need to decide whether to proceed with a routing decision.
- `accept_assessment(assessment)` — validates a single `CandidateAssessment` (installed + evidence present). Used when evaluating individual candidates.

Both share the `RejectionReason` type.

### `report.rs` contracts

**Consumers serialize `RouteDecisionReport`, never `RoutingTrace` directly.**
`RouteDecisionReport` uses string labels for all enum fields — decouples JSON shape from internal enum changes.

- **Do not construct `RouteDecisionReport` by hand** — use `RouteDecisionReport::from_trace(trace)`.
- `RouteSummaryReport` is a compact subset for CLI JSON output.

### Link filtering rule

Only `KnownHarness` links (from `config::targets::normalize_link`) filter routing candidates.
Generic targets (`.agents`, `agents`, unknown names) and path-like targets are **invisible**
to routing — they are materialization-only. See `config::targets` for normalization details.

When known linked harnesses exist:
- Auto-routing candidates are filtered to the linked set before evaluation
- `settings.default_harness` outside the linked set is ignored (with diagnostic)
- Hardcoded fallback is blocked (linked harnesses select themselves instead)

## Architecture

```
RoutingInput
    │
    ├─ settings_harness_order? → parse + link-filter → ConfigOrder candidates
    ├─ (no order) → provider_candidate_order → link-filter → Provider candidates
    │
    └─ for each candidate:
           not installed              → skip (not_installed)
           native provider + auth     → Confirmed ✓
           native + constraint mismatch → skip (provider_constraint_unsatisfied)
           opencode + probe success   → Confirmed/Constrained ✓
           opencode + no model match  → skip (no_model_match)
           pi + compatible probe      → Confirmed/Constrained ✓
           pi + incompatible probe    → skip (pi_incompatible)
           pi + no probe              → Passthrough ✓
           cursor                     → Passthrough ✓
           else                       → skip (unsupported_candidate)

    exhausted candidates → config_default_harness → linked fallback → hardcoded pi
                           (link constraints can block each of these)

Module boundaries:
    slug.rs       ← stable root: borrowed parsing, normalized matching
    acceptance.rs ← policy layer: MatchPolicy, RejectionReason
    report.rs     ← serialization DTO: RouteDecisionReport, string labels
    mod.rs        ← evaluator: RoutingInput → RoutingTrace
```

## Rationale

Single evaluator prevents `mars models` and `mars build` from drifting on
routing decisions. Before this module, both had independent candidate evaluation
logic that could diverge on harness ordering, auth gates, and probe handling.

**SelectionKind vs MatchEvidence split:** the old `RouteConfidence` conflated
"how was this selected" with "what evidence supports it". Fixed selections
were forced into `Explicit` confidence, losing the actual evidence level.
Now `SelectionKind::Fixed` answers the selection question and the assessment's
`MatchEvidence` preserves the actual evidence — callers get both dimensions.

**slug.rs extracted as stable root:** slug matching was duplicated between
`routing/mod.rs` and `models/availability.rs`. Extracting it eliminates drift
and gives both modules a single source of truth for provider/model parsing.
Borrowed `SlugParts` avoids allocation in hot scanning loops.

**report.rs decouples JSON from internals:** `RouteDecisionReport` uses string
labels so new evaluator variants don't break serialized output. Consumers
serialize the report, never the internal `RoutingTrace`.

Link constraints blocking hardcoded/config-default fallback is intentional:
`settings.targets = [".opencode"]` signals project intent to use OpenCode.
Silently routing to Claude as a fallback contradicts that intent.

Pi upgrade from Passthrough→Confirmed: before PR #51, a Pi binary on PATH was
always Passthrough (unknown capability). With the Pi probe, Mars knows whether
the installed Pi supports the required spawn flags, so it can express Confirmed
confidence.

Route facts (`Passthrough` evidence, `provider-match` source, `unknown`
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

**Build a fixed-selection trace:**

```rust
let assessment = evaluate_fixed_harness(&input, "codex");
let trace = trace_for_fixed_harness(RouteSource::Cli, "codex", assessment, diagnostics);
```

**Check acceptance:**

```rust
// Full trace against policy
accept_route(&trace, &installed_harnesses, MatchPolicy::RequireSlugEvidence)?;

// Single candidate assessment
accept_assessment(&assessment)?;
```

**Serialize for CLI output:**

```rust
let report = trace.to_report(); // or RouteDecisionReport::from_trace(&trace)
let json = serde_json::to_string(&report)?;
```

**Parity smoke test** (run in a temp project with known config):

```bash
HARNESS=$(mars models resolve gpt-5.4-mini --json | jq -r '.harness')
BUNDLE_HARNESS=$(mars build launch-bundle --model gpt-5.4-mini --json | jq -r '.routing.harness')
[ "$HARNESS" = "$BUNDLE_HARNESS" ] || echo "DRIFT"
```
