# src/build/policy/ — Policy Resolution

Resolves routing and execution policy for a launch bundle. 5 files, ~2700 lines.

## Mental Model

Each field resolves independently through its own module, then results combine
in `mod.rs`:

```
resolve_policy()
  ├─ model::resolve_model()             → model_id, provider, model_token
  ├─ harness::resolve_harness()         → harness, route trace
  ├─ execution::resolve_execution_policy() → effort, approval, sandbox, autocompact
  └─ runnable::resolve_routing()        → final Routing struct (warnings always empty)
```

## Field Independence

CLI model and harness pins constrain separate dimensions. A model pin disables
model backups; a harness pin restricts every model attempt to that harness.
Overlay/profile/model-policy/alias harness fields are ordered preferences, not
pins. Assess the highest-precedence preference first, then normal harness order.
Runtime inability yields to other permitted candidates; invalid configuration
remains fatal. Never clear a requested model to make a harness work.

## Key Rules

- `resolve_routing()` returns `warnings: Vec::new()` always — route facts go to
  `routing.harness_model_source` / `routing.harness_model_confidence`, not warnings
- Catalog refresh (`ensure_fresh`) runs before harness evaluation, not read-only

## Target Permission

Every harness preference must remain inside the configured `HarnessScope`
minus `PolicyInput.excluded_harnesses`. Caller exclusions narrow permission; they
do not replace target configuration. Build CLI accepts repeatable `--exclude-harness`.
Excluded CLI pins fail; excluded implicit preferences yield to permitted routes.
The common routing assessor also rejects disabled candidates before auth probes.
Automatic exhaustion returns an error; build never substitutes an uninstalled
first candidate. Config defaults and remaining permitted harnesses use normal assessment.

Native auth observations use one command-scoped NativeAuthCache across primary
and backup evaluations. Support evidence alone does not establish runtime eligibility.

## Profile Model Fallback

Primary first, then all concrete profile policy entries in declaration order.
Select the first eligible model route; defer the first unverified attempt until
all candidates are exhausted. Keep its model, settings and provenance together.
An explicit model pin prevents backup enumeration.
`no-fallback` excludes only its entry, never the whole chain. Settings matching
remains overlay → profile → settings, independent of candidate enumeration.
`fallback_model_policy_entries()` is shared with backup inventory; native fanout
keeps its broader flagged-entry/glob semantics and must not use this helper.

## Anti-Patterns

- Do NOT add route-path facts to the warnings vector
- Do NOT assume model and harness come from the same precedence source

## See Also

- `.context/CONTEXT.md` — preference ordering and exhaustion
- `../AGENTS.md` — bundle construction pipeline (parent context)
- `../../routing/AGENTS.md` — harness candidate evaluation and probe matching
- `../../models/AGENTS.md` — catalog refresh, model alias resolution
