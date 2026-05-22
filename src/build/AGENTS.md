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

### Ad-hoc Mode (`--model`, no `--agent`)
- Works from a plain directory with **no `mars.toml`**
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
config::load_policy_resolution_config()  → aliases, harness_order, linked targets
model::resolve_model()                   → alias → model_id + provider
harness::resolve_harness()               → route selection, candidate eval
execution::resolve_execution_policy()    → effort, approval, sandbox, autocompact
runnable::resolve_routing()              → populate Routing (warnings always empty)
```

## Anti-Patterns

- Do NOT add warnings for harness-model path facts (`passthrough`, `synthesized`, `unknown` confidence)
- Do NOT call `build_launch_bundle` with agent name from outside synced project
- Do NOT emit `eprintln!` in library code

## See Also

- `.context/CONTEXT.md` — detailed contracts, warning semantics, ad-hoc guard
- `src/routing/AGENTS.md` — harness candidate evaluation
- `src/cli/build.rs` — CLI arg definitions
