# src/routing/ — Harness Candidate Evaluation

Single candidate evaluator for harness selection. 4 files + `.context/`, ~2200 lines.

## Mental Model

```
RoutingInput → evaluate_candidates() → RoutingTrace → accept_route() → decision
```

**Single evaluator invariant:** `evaluate_candidates()` is the **only** candidate evaluator. Both `mars models` and `mars build` call it — this is what makes routing outputs consistent.

## Module Layout

| File | Responsibility |
|---|---|
| `mod.rs` | Evaluator: `RoutingInput` → `RoutingTrace` |
| `slug.rs` | Borrowed slug parsing, normalized matching |
| `acceptance.rs` | Policy layer: `MatchPolicy`, `RejectionReason` |
| `report.rs` | Serialization DTO: `RouteDecisionReport` |

## Evaluation Flow

1. Rank candidates by configured harness order (registry default when unset), then
   `default_harness`, then remaining registry harnesses; deduplicate stably.
2. Intersect candidates with target permission and caller exclusions before probes.
3. Assess installation and support, then applicable native auth. Auth callbacks
   return `AuthState`; command-scoped `NativeAuthCache` preserves unknown results.
4. Prefer eligible routes; defer the first unverified route until all harnesses
   for this model have been assessed. Blocked routes never become fallback routes.

### Default `harness_order`

When `settings.harness_order` is omitted, policy loaders supply
`harness::registry::default_harness_order_names()` — canonical list and rationale live in
[`src/harness/registry.rs`](../harness/registry.rs) (`DEFAULT_HARNESS_ORDER`).

Reported harness-order positions index the normalized valid-name list (invalid
entries have already been removed), not the authored configuration array.

### Eligibility is not support evidence

`CandidateAssessment::eligibility()` distinguishes eligible, unverified and blocked.
A native auth timeout remains unverified; a known rejection is blocked. Universal
model-list/probe success proves support, not account authentication or quota.
No-model native launches still check auth. Native materialization supplies
`AuthState::NotApplicable` and accepts support without probing runtime accounts.

### Routing parity with `mars models` and launch-bundle

`mars models list|resolve` and `mars build launch-bundle` both call `evaluate_candidates()`
with the same `RoutingInput` shape: shared capability snapshot, probe caches, and
`catalog_model_slugs` for native harness matching. Parity drift is a bug — see parity smoke
in `.context/CONTEXT.md`.

## Key Types

### `SelectionKind` (how selected)
| Value | Meaning |
|---|---|
| `Auto` | First acceptable from candidate loop |
| `Fixed` | Caller committed to specific harness |

### `MatchEvidence` (what supports it)
| Value | Meaning |
|---|---|
| `Confirmed` | Native model/provider match or positive harness support probe |
| `Constrained` | Provider-constrained support evidence |
| `Passthrough` | Universal harness or Pi without probe |
| `None` | No support evidence |

### `MatchPolicy` (acceptance strictness)
| Policy | Accepts |
|---|---|
| `RequireSlugEvidence` | `Confirmed` or `Constrained` only |
| `AllowPassthrough` | `Confirmed`, `Constrained`, or `Passthrough` |

Both policies reject blocked assessments; neither installation nor support evidence
can override an authentication rejection.

## Link Filtering

`permission_denial` applies configured scope and caller exclusions before evidence
collection. It distinguishes `disabled_target` from `excluded_by_caller`; neither
changes the recorded physical installation state. Automatic fallback candidates
also remain inside this intersection.

Configured targets permit only their known harnesses. Empty or generic/path-only
configuration permits none; only absent targets and managed_root are unrestricted.
The shared assessor rejects disabled fixed routes before installation/auth/support
checks. Build policy rejects an excluded CLI pin and skips excluded preferences.

## Patterns

**Test without real auth:**
```rust
let trace = evaluate_candidates_with_auth(&input, |_harness| AuthState::Authenticated);
```

**Simulate Pi compatibility:**
```rust
let pi_probe = PiProbeResult { compatible: true, ..PiProbeResult::default() };
```

**Check acceptance:**
```rust
accept_route(&trace, &installed, MatchPolicy::RequireSlugEvidence)?;
```

## See Also

- `.context/CONTEXT.md` — detailed contracts, slug semantics, report serialization
- `src/harness/.context/CONTEXT.md` — harness registry and capability snapshot
- `src/config/AGENTS.md` — target normalization and `HarnessScope`
