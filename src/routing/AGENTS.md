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

1. Build candidate list from `settings_harness_order` (when unset, see default order below) or `provider_candidate_order`
2. Apply `HarnessScope` before probes; empty `Only` means no candidates
3. Per-candidate gate: installed → native catalog slug match + auth → OpenCode probe → Pi probe → Pi/Cursor passthrough (deferred)
4. Fallback chain: config `default_harness` → linked fallback → no selection
5. Link constraints block config-default fallback from routing outside known links

### Default `harness_order`

When `settings.harness_order` is omitted, policy loaders supply
`harness::registry::default_harness_order_names()` — canonical list and rationale live in
[`src/harness/registry.rs`](../harness/registry.rs) (`DEFAULT_HARNESS_ORDER`).

### Deferred passthrough (Pi, Cursor)

`Passthrough` candidates do **not** win immediately. The loop records the first passthrough
harness and keeps trying stronger candidates (`Confirmed` / `Constrained` exit early). Only
after the order is exhausted does Mars return that deferred passthrough selection. This lets
native and probe-backed harnesses outrank universal routers.

### Routing parity with `mars models` and launch-bundle

`mars models list|resolve` and `mars build launch-bundle` both call `evaluate_candidates()`
with the same `RoutingInput` shape: shared capability snapshot, probe caches, and
`catalog_model_slugs` for native harness matching. Parity drift is a bug — see parity smoke
in `.context/CONTEXT.md`.

### Linked fallback and prior skips

When auto-routing exhausts candidates under link constraints, `select_linked_fallback_harness`
walks linked harnesses in `harness_order` (or link declaration order) and **skips** harnesses
whose assessment has a hard `skip_reason` (`not_installed`, `pi_incompatible`, `no_model_match`,
etc.). Soft passthrough deferrals do not block linked fallback the same way.

## Key Types

### `SelectionKind` (how selected)
| Value | Meaning |
|---|---|
| `Auto` | First acceptable from candidate loop |
| `Fixed` | Caller committed to specific harness |
| `ConfigDefault` | Fell through to `settings.default_harness` |
| `LinkedFallback` | Linked harnesses selected themselves |

### `MatchEvidence` (what supports it)
| Value | Meaning |
|---|---|
| `Confirmed` | Native provider match + authenticated, or compatible Pi probe |
| `Constrained` | Same as Confirmed, but provider_constraint was active (includes cursor with provider constraint when probe can't confirm) |
| `Passthrough` | Universal harness, Pi without probe, config-default fallback |
| `None` | Rejected candidate |

### `MatchPolicy` (acceptance strictness)
| Policy | Accepts |
|---|---|
| `RequireSlugEvidence` | `Confirmed` or `Constrained` only |
| `AllowPassthrough` | `Confirmed`, `Constrained`, or `Passthrough` |
| `InstalledOnly` | Any evidence, as long as harness is installed |

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
let trace = evaluate_candidates_with_auth(&input, |_harness| true);
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
