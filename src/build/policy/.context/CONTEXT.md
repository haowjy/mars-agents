# src/build/policy/

## Contracts

### Cross-field precedence soft-fail

When a fixed harness conflicts with a model from a lower-precedence source, the harness
wins and the model is cleared. Precedence ranks (`PolicySource::precedence_rank()`):

| Rank | Sources |
|------|---------|
| 5 | CLI |
| 4 | Overlay, OverlayModelPolicy |
| 3 | Profile, ProfileModelPolicy, ProfileHarnessOverride |
| 2 | SettingsModelPolicy, Project, Config |
| 1 | Alias |
| 0 | Unset, ConfigOrder, Provider, Default |

**Trigger condition:** fixed harness assessment returns `skip_reason = "no_model_match"` AND
`harness_source.precedence_rank() > model_source.precedence_rank()`.

**Only `no_model_match` triggers soft-fail.** Other rejection reasons (`provider_constraint_unsatisfied`,
`pi_incompatible`) stay hard errors regardless of precedence rank — clearing the model cannot
resolve a provider constraint or harness compatibility failure.

**Same-precedence conflicts are hard errors.** CLI harness + CLI model both `no_model_match` →
error; the user explicitly requested an impossible combination.

### model_override mechanism

`HarnessResolution.model_override: Option<()>` signals the outcome. When `Some(())`, `mod.rs`
substitutes empty strings for `model`, `model_token`, `provider_constraint`, and `provider_for_order`
before calling `runnable::resolve_routing` — the harness uses its own default model. A warning
(`"<source> model '<token>' cannot run on <source> harness '<name>'; clearing model"`) flows
through the normal warnings pipeline to the bundle's `warnings[]` field.

### Resolution flow in harness.rs

```
resolve_harness()
  ├─ resolve_fixed_harness_selection()   → Option<ResolvedField> (CLI > overlay > profile > alias)
  ├─ [fixed] evaluate_fixed_harness      → assessment
  │   ├─ [profile + not installed]       → pivot to candidate evaluation
  │   ├─ [assessment rejected]           → resolve_fixed_harness_rejection()
  │   │   ├─ not installed               → hard error
  │   │   ├─ no_model_match + outranks   → soft_fail (clear model, retry with empty model_id)
  │   │   └─ other / same rank           → hard error
  │   └─ [assessment ok]                 → use fixed trace
  └─ [auto] evaluate_candidates          → candidate ordering with probe matching
```

## Rationale

Cross-field precedence conflict arises from harness shortcuts (`meridian opencode`) that
set CLI-level harness while the agent profile provides a model. Before this mechanism,
this was a hard error requiring the user to also override the model. The soft-fail lets
the higher-precedence field win naturally — the harness proceeds with its own default
model selection, and the warning tells the user what happened.

Only `no_model_match` is soft-failed because it means "the harness can't find this model
in its probe results" — clearing the model is a valid recovery (let the harness pick).
Provider constraints and harness incompatibility are structural — clearing the model
doesn't help.

## Related docs

- `../AGENTS.md` — policy resolution mental model and field independence
- `../../.context/CONTEXT.md` — bundle-level contracts, warning semantics
- `../../../routing/.context/CONTEXT.md` — candidate evaluation, assessment mechanics
