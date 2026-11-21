# Launch policy state machines

The index of all machines is [state-machines/index.md](../../../../.context/state-machines/index.md).
The policy layers and their order are in [CONTEXT.md](CONTEXT.md).

## Launch policy

One launch request's model and harness, as two loops: the outer over model candidates, the inner
over harnesses. `resolve_policy` (`mod.rs`) runs the candidates and the loop and then selects;
`resolve_harness` (`harness.rs`) takes the pinned branch (`HarnessAttempt`) or calls
`routing::evaluate_candidates`. Candidate evaluation and acceptance ordering are in
[harness routing](../../../routing/.context/state-machines.md); this machine decides what to try
and what a failure means.

```mermaid
stateDiagram-v2
  [*] --> model_candidates
  model_candidates --> harness_routing: next candidate model, in declaration order
  harness_routing --> pinned_attempt: CLI harness pin, or profile/overlay/alias preference
  pinned_attempt --> selected: accepted
  pinned_attempt --> error_pin: rejected and the pin was the CLI's
  harness_routing --> selected: an eligible route
  harness_routing --> deferred: a route chosen but unverified
  harness_routing --> model_candidates: no route, next model
  deferred --> model_candidates: remember the first, keep trying
  model_candidates --> deferred_selected: nothing eligible left
  model_candidates --> error_exhausted: nothing at all
  selected --> [*]
  deferred_selected --> [*]: warning with the fallback
  error_pin --> [*]
  error_exhausted --> [*]
  harness_routing --> error_invalid: config error on a candidate
  error_invalid --> [*]
```

An instance of [an ordered fallback walk](../../../../.context/state-machines/fallback-walk.md).

- **`no-fallback` excludes its own entry, not the chain.**
- **A CLI model pin disables backups.** A CLI harness pin restricts every attempt to that harness
  and, if target scope excludes it, is an error.
- **An unverified route is deferred, not accepted**, and the whole attempt is kept so later
  candidates do not overwrite its settings.
- **Exhaustion is an error.** Nothing is substituted unassessed.
- Nothing persists. The outcome goes into the launch bundle as `routing` and `provenance`.
- `PolicySource` (`mod.rs`) and the routing layer's `RouteSource` have the same variants with a
  `From` impl in both directions; add a variant to both.

## Effort resolution

`runnable::resolve_routing` resolves the layers' effort request once the harness model is known:
the catalog row of the provider the harness calls (`models::reasoning::catalog_identity` and
`catalog_row`; one join per harness), then `mars_schema::effort::resolve_effort` over the model's levels and the harness's. A
move is a `launch.warning`. The same row goes into the bundle as `routing.catalog_row`, so
consumers price from it and never re-join. Sync does the same in `profile_for_model`, per harness.

A harness with `SlugShape::Bare` (`agy`) takes model and effort as one slug, so `resolve_routing`
asks `models::slug_tiers::find` what its listing says the model is, and `pick_tier` resolves the
effort over the family's listed tiers (not the harness's static levels) and returns the slug that
carries it. `harness_model` is that slug and `effort` is the tier it carries; no effort sends the
bare family, one tier always runs, and an untiered slug sends no effort. The effort a tiered `-m`
slug carries sits at the precedence of the layer that named the model (`layer_rank` in `mod.rs`):
an effort at that layer or above overrides it with a warning. A move never fails the launch. With no
usable listing the model passes through and an effort is dropped with a warning.
