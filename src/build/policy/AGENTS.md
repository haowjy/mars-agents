# src/build/policy/ — Policy Resolution

Resolves routing and execution policy for a launch bundle. 5 files, ~2700 lines.

## Mental Model

Each field resolves independently through its own module, then results combine
in `mod.rs`:

```
resolve_policy()
  ├─ model::resolve_model()             → model_id, provider, model_token
  ├─ harness::resolve_harness()         → harness, route trace, model_override
  ├─ execution::resolve_execution_policy() → effort, approval, sandbox, autocompact
  └─ runnable::resolve_routing()        → final Routing struct (warnings always empty)
```

## Field Independence and Cross-Field Conflict

Fields resolve independently — CLI model does not force CLI harness or vice versa.
But independently-resolved fields can **conflict**: a profile model may not run on
a CLI harness. When this happens, precedence rank decides the outcome:

- **Harness outranks model** → model cleared with warning, harness proceeds
  (soft-fail, `no_model_match` only)
- **Same or model outranks** → hard error (user asked for an impossible combination)
- **Non-model rejections** (provider constraint, incompatibility) → always hard error

See `.context/CONTEXT.md` for precedence ranks and the soft-fail contract.

## Key Rules

- `resolve_routing()` returns `warnings: Vec::new()` always — route facts go to
  `routing.harness_model_source` / `routing.harness_model_confidence`, not warnings
- Harness resolution may clear the model (`model_override: Some(())`); downstream
  routing must check this before using resolved model fields
- Catalog refresh (`ensure_fresh`) runs before harness evaluation, not read-only

## Anti-Patterns

- Do NOT add route-path facts to the warnings vector
- Do NOT assume model and harness come from the same precedence source
- Do NOT bypass `model_override` — if harness resolution cleared the model, routing
  must use empty model fields

## See Also

- `.context/CONTEXT.md` — precedence ranks, soft-fail contract, model_override mechanism
- `../AGENTS.md` — bundle construction pipeline (parent context)
- `../../routing/AGENTS.md` — harness candidate evaluation and probe matching
- `../../models/AGENTS.md` — catalog refresh, model alias resolution
