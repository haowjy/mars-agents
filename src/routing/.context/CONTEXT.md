# src/routing/

Multi-file module: `mod.rs` (evaluator), `slug.rs` (slug matching), `acceptance.rs` (policy), `report.rs` (serialization). The canonical candidate evaluator for harness selection.

## Contracts

### Single evaluator invariant

`evaluate_candidates()` is the **only** candidate evaluator in the codebase.
Both `mars models` (resolve/list) and `mars build launch-bundle` call it —
this is what makes their routing outputs consistent. A second evaluator anywhere
would break the parity invariant.

`evaluate_fixed_harness_with_auth_and_probes()` evaluates one specific harness
without fallback, using the caller's probe resolver and authentication check.
It is used when the caller has already committed to a fixed harness choice
(CLI `--harness`, profile `harness:`, alias `harness:`). It returns a single
`CandidateAssessment` — the caller decides what to do with a failed fixed
selection.

**The evaluator never errors** — `evaluate_candidates()` always returns a `RoutingTrace`.
Acceptance decisions belong to callers via `accept_route()` / `accept_assessment()`.

### `RoutingInput` fields

| Field | Role |
|---|---|
| `model_id` | Resolved model identifier (used for OpenCode/Pi slug matching) |
| `provider_for_order` | Optional provider name for native compatibility and model-slug preference |
| `provider_constraint` | Alias/provider pin from model config — filters probe slug selection and native harness acceptance; shapes `harness_model` via [`resolve_harness_model`](../../models/harness_model.rs) (no blind `provider/model` prefix) |
| `settings_provider_order` | Raw `provider_order` from config, if set |
| `settings_harness_order` | Raw `harness_order` from config, if set |
| `config_default_harness` | Raw `default_harness` from config, if set |
| `installed_harnesses` | Set of harness names found on PATH |
| `excluded_harnesses` | Caller restrictions, independent of configured target permission |
| `harness_scope` | `Unrestricted` or `Only(BTreeSet<HarnessId>)`; empty denies all routes |
| `opencode_probe_result` | Cached OpenCode probe (provider/model evidence) |
| `pi_probe_result` | Cached Pi probe (binary + help-surface compatibility) |
| `catalog_model_slugs` | Cached models.dev `provider/model` slugs; native harnesses match here before auth-only fallback |

When `settings.harness_order` is unset, `config/routing_settings` and `build/policy`
inject [`default_harness_order_names()`](../../harness/registry.rs) — see
[`src/harness/registry.rs`](../../harness/registry.rs) (`DEFAULT_HARNESS_ORDER`) for the
canonical ordered list.

### Deferred unverified routes

The evaluator ranks `CandidateAssessment::eligibility()`, independently of support
match strength. Native authentication yields eligible only after support succeeds;
unknown native auth and missing universal auth proof remain unverified. Keep the
first unverified route with its original support evidence and provenance, then
assess remaining harnesses for an eligible route. A blocked route cannot be deferred.
This is within-model ranking; the outer build policy owns cross-model traversal.

### Native catalog slug matching

For native harnesses (Claude, Codex), when `catalog_model_slugs` is populated (from
`models::ensure_fresh` + `catalog_model_slugs`), `candidate_match_evidence` uses
`select_probe_slug` over catalog entries filtered to that harness's provider prefix.
Empty catalog falls back to provider-native affinity + auth gate only.

### `SelectionKind` semantics

| Value | Meaning |
|---|---|
| `Auto` | Selected by candidate evaluation loop (first acceptable harness) |
| `Fixed` | Caller committed to a specific harness (CLI/profile/alias) |

### `MatchEvidence` semantics

| Value | Evidence |
|---|---|
| `Confirmed` | Native model/provider match, compatible Pi probe, or positive harness model probe |
| `Constrained` | Same as Confirmed, but a `provider_constraint` was active |
| `Passthrough` | Universal harness (Cursor), Pi without fresh probe, OpenCode unknown-provider |
| `None` | No evidence — candidate was rejected |

`RouteSource` records preference provenance, `SelectionKind` distinguishes automatic
from fixed selection, and `MatchEvidence` describes support evidence. A config-default
candidate is assessed in the same automatic loop; its source gives it no authority
to bypass a failed assessment.

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

**`accept_route()` vs `accept_assessment()`:**
- `accept_route(trace, installed, policy)` — validates a full `RoutingTrace` against a policy. Used by callers who need to decide whether to proceed with a routing decision.
- `accept_assessment(assessment)` — validates a single `CandidateAssessment` (not blocked; support and auth remain separate). Used when evaluating individual candidates.

Both share the `RejectionReason` type and reject blocked assessments. A present
`MatchEvidence::None` is not support. Native auth rejection cannot be accepted
merely because a matching slug and an installed binary exist.

### `report.rs` contracts

**Consumers serialize `RouteDecisionReport`, never `RoutingTrace` directly.**
`RouteDecisionReport` uses string labels for all enum fields — decouples JSON shape from internal enum changes.

- **Do not construct `RouteDecisionReport` by hand** — use `RouteDecisionReport::from_trace(trace)`.
- `RouteSummaryReport` is a compact subset for CLI JSON output.

### Link filtering rule

Target permission comes from `config::targets::HarnessScope`. Generic/path targets
add no harnesses; an explicitly empty scope must not become unrestricted. Fixed
and automatic assessments reject disabled routes before any auth/support probes.
Build rejects excluded CLI harness pins and skips excluded implicit preferences.

The automatic list starts with configured order (registry default if unset), then
config default, then remaining registry harnesses. Permission filters and stable
deduplication apply before assessment. No rejected candidate is retried or promoted;
no installed executable means no selected route.

## Architecture

```text
scope + exclusions → ordered/deduplicated harnesses
    → support assessment → applicable typed auth observation
    → eligible: select / unverified: defer / blocked: skip
    → RoutingTrace → acceptance policy → RouteDecisionReport
```

Auth callbacks are command-scoped: `NativeAuthCache` is shared across aliases and
model attempts. Native compilation injects NotApplicable, preserving support-only
materialization without account probes. Report verdict/reason labels never include
raw AuthState::Unknown details or auth command output.

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

Permission applies to every ranked candidate:
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
let trace = evaluate_candidates_with_auth(&input, |_harness| AuthState::Authenticated);
```

**Simulate Pi compatibility:**

```rust
let pi_probe = PiProbeResult { compatible: true, ..PiProbeResult::default() };
// pass Some(&pi_probe) as pi_probe_result in RoutingInput
```

**Build a fixed-selection trace:**

```rust
let assessment = evaluate_fixed_harness_with_auth_and_probes(
    &input,
    "codex",
    probe_resolver,
    auth_check,
);
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

## Related docs

- [src/models/AGENTS.md](../../models/AGENTS.md) — `ensure_fresh`, `ModelsRefreshControl`, catalog TTL
- [src/harness/registry.rs](../../harness/registry.rs) — `DEFAULT_HARNESS_ORDER`, `default_harness_order_names`
- [src/build/.context/CONTEXT.md](../../build/.context/CONTEXT.md) — launch-bundle `harness_model`, effort baking

**Parity smoke test** (run in a temp project with known config):

```bash
HARNESS=$(mars models resolve gpt-5.4-mini --json | jq -r '.harness')
BUNDLE_HARNESS=$(mars build launch-bundle --model gpt-5.4-mini --json | jq -r '.routing.harness')
[ "$HARNESS" = "$BUNDLE_HARNESS" ] || echo "DRIFT"
```
