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

1. Build candidate list from `settings_harness_order` or `provider_candidate_order`
2. Filter by `linked_harnesses` — only `KnownHarness` links filter
3. Per-candidate gate: installed → native match + auth → OpenCode probe → Pi probe → Cursor passthrough
4. Fallback chain: config `default_harness` → linked fallback → hardcoded `pi`
5. Link constraints block config-default and hardcoded fallbacks from routing outside known links

## Key Types

### `SelectionKind` (how selected)
| Value | Meaning |
|---|---|
| `Auto` | First acceptable from candidate loop |
| `Fixed` | Caller committed to specific harness |
| `ConfigDefault` | Fell through to `settings.default_harness` |
| `LinkedFallback` | Linked harnesses selected themselves |
| `HardcodedDefault` | Defaulting to `pi` |

### `MatchEvidence` (what supports it)
| Value | Meaning |
|---|---|
| `Confirmed` | Native provider match + authenticated, or compatible Pi probe |
| `Constrained` | Same as Confirmed, but provider_constraint was active |
| `Passthrough` | Universal harness, Pi without probe, config-default fallback |
| `None` | Rejected candidate |

### `MatchPolicy` (acceptance strictness)
| Policy | Accepts |
|---|---|
| `RequireSlugEvidence` | `Confirmed` or `Constrained` only |
| `AllowPassthrough` | `Confirmed`, `Constrained`, or `Passthrough` |
| `InstalledOnly` | Any evidence, as long as harness is installed |

## Link Filtering

Only `KnownHarness` links (from `config::targets::normalize_link`) filter routing candidates. Generic targets (`.agents`, unknown names) and path-like targets are **invisible** to routing.

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
- `src/config/AGENTS.md` — link normalization that produces `linked_harnesses`
