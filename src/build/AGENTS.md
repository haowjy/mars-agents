# src/build/ — Launch Bundle Construction

Generates serializable launch bundles for harness runtimes. 6 files + `.context/`, ~2700 lines.

## Mental Model

```
LaunchBundleRequest {agent, model, harness, effort, approval, sandbox, extra_skills}
    ↓
[agent mode] read .mars/agents/<name>.md → AgentProfile + body
[ad-hoc mode] empty_agent_profile() + "" body
    ↓
resolve_policy() → Routing + ExecutionPolicy + Provenance
    ↓
resolve_effective_skills() + compile_prompt_surface() + resolve_bundle_tools()
    ↓
LaunchBundle {routing, execution_policy, prompt_surface, tools, skills_metadata, provenance, warnings}
```

## Two Modes

### Ad-hoc Mode (no `--agent`)
- Works from a plain directory with **no `mars.toml`**
- `--model` is optional; when omitted, the selected harness receives an empty
  model field and uses its own default model
- `can_run_without_project()` in `src/cli/mod.rs` supplies synthetic `MarsContext`
- No skills loaded, no tools resolved (empty sets)

### Agent/Profile Mode (`--agent <name>`)
- Reads `.mars/agents/<name>.md` from `ctx.project_root`
- Requires synced project (`mars.toml` present, `mars sync` already run)
- Full profile parsing + skill loading + tool resolution

## Warning Semantics

`LaunchBundle.warnings[]` contains **user-actionable degraded states only**.

**REAL warnings** (user can act on them):
- Linked harness constraints blocked auto-routing
- Cursor is experimental target
- Unknown tool name on first-class harness

**NOT warnings** (go to routing/provenance fields):
- `harness_model_source: "passthrough"` — expected behavior
- `harness_model_confidence: "unknown"` — correct answer for passthrough harnesses

`resolve_routing()` always returns `warnings: Vec::new()`. Route facts go to `routing.harness_model_source` and `routing.harness_model_confidence`.

## Policy Resolution Pipeline

```
build_launch_bundle()                    → load EffectiveProjectConfig once
                                        (fallback to default only when mars.toml is absent)
models::merged_runtime_aliases()         → merged alias map (dependency + consumer overlays)
models::ensure_fresh()                   → models.dev catalog (TTL-aware; not read-only)
model::resolve_model()                   → alias → model_id + provider, or unset
harness::resolve_harness()               → route selection, candidate eval (shared evaluator);
                                           cross-field soft-fail: fixed harness outranks lower-
                                           precedence model on no_model_match → model cleared
execution::resolve_execution_policy()    → effort, approval, sandbox, autocompact
runnable::resolve_routing()              → populate Routing (warnings always empty)
```

`resolve_policy` always runs `models::ensure_fresh` on `.mars/` (stale fallback may warn).
CLI passes `ModelsRefreshControl` from `--refresh-models` / `--no-refresh-models` on
`build launch-bundle` — flag matrix and probe modes: [`src/models/AGENTS.md`](../models/AGENTS.md),
`models::resolve_models_refresh_control`.

### Cursor `harness_model` contract

For harness `cursor`, `runnable::resolve_routing` may bake CLI/profile **effort** into
`routing.harness_model` via the Cursor probe slug list (`medium` / `none` / `auto` / `default`
→ unsuffixed base slug; other tiers → suffixed slug). On success, `routing.effort` is cleared
(`effort_consumed`). Downstream adapters must run **`harness_model` verbatim** — not re-derive
effort from `routing.effort`.

`routing.candidate_slugs` is **diagnostic only** (probe/catalog candidates for the selected
harness). Consumers ignore it unless debugging.

Alias `provider` → `harness_model` (bare native ids vs probe slugs): [`src/models/.context/CONTEXT.md`](../models/.context/CONTEXT.md).

## Anti-Patterns

- Do NOT add warnings for harness-model path facts (`passthrough`, `synthesized`, `unknown` confidence)
- Do NOT call `build_launch_bundle` with agent name from outside synced project
- Do NOT emit `eprintln!` in library code

## See Also

- `.context/CONTEXT.md` — detailed contracts, warning semantics, ad-hoc guard
- `src/routing/AGENTS.md` — harness candidate evaluation
- `src/cli/build.rs` — CLI arg definitions
