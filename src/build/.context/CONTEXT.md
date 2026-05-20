# src/build/

Generates launch bundles — the serializable artifact that a harness runtime consumes to
launch an agent with resolved routing, execution policy, tools, and prompt surface.

## Contracts

### Two launch-bundle modes

1. **Ad-hoc mode** (`--model`, no `--agent`):
   - Requires `--model`; `--agent` must be absent
   - Works from a plain directory with **no `mars.toml`** — `can_run_without_project()`
     in `src/cli/mod.rs:248` supplies a synthetic `MarsContext` when project
     discovery fails and the exact ad-hoc condition is met
   - `empty_agent_profile()` is used (no agent file read, no skills, no tools)
   - Skills loading skipped (no `.mars/` store needed); tool resolution yields empty sets

2. **Agent/profile mode** (`--agent <name>`):
   - Reads `.mars/agents/<name>.md` from `ctx.project_root`
   - Requires a synced project (`mars.toml` present, `mars sync` already run)
   - Parses frontmatter YAML + markdown body from the agent file
   - Profile fields (model, harness, skills, tools, etc.) feed into policy resolution

### Warning semantics

`LaunchBundle.warnings[]` contains **user-actionable degraded states only**.
Harness-model path facts (`passthrough`, `synthesized`, `unknown` confidence,
`provider-match`, `cached-probe`) belong in `routing.*` and `provenance.*` fields —
they are expected route metadata, not problems a user can fix.

`src/build/policy/runnable.rs` enforces this at the outer boundary:
`resolve_routing()` always returns `warnings: Vec::new()`. Route facts are recorded
in `routing.harness_model_source` and `routing.harness_model_confidence`. The caller
layer (`policy/mod.rs`) owns warning promotion for actual degraded states.

Examples of REAL warnings (user can act on them):
- `"known linked harness constraints left no eligible auto-routing candidates; selecting linked harness \`codex\` without unrelated fallback"` → user should check `settings.targets`
- `"Cursor is an experimental launch-bundle target. The contract may change without notice."` → user is informed of instability risk
- `"tool 'X' is not a known <harness> tool; passing through verbatim"` → tool normalization couldn't resolve the name; user may have a typo

Examples that are NOT warnings (they go to routing/provenance fields):
- `harness_model_source: "passthrough"` — Pi or explicit harness receives the model token as-is; this is expected behavior
- `harness_model_confidence: "unknown"` — Pi/passthrough harnesses are provider-routers at runtime; unknown confidence is the correct answer

### `can_run_without_project` guard

```rust
// src/cli/mod.rs:248
fn can_run_without_project(cmd: &Command, err: &MarsError) -> bool {
    matches!(
        (cmd, err),
        (
            Command::Build(build::BuildArgs {
                command: build::BuildCommand::LaunchBundle(build::LaunchBundleArgs {
                    agent: None,
                    model: Some(_),
                    ..
                })
            }),
            MarsError::Config(ConfigError::ProjectRootNotFound { .. })
        )
    )
}
```

Only ad-hoc launch-bundle (`--agent` absent, `--model` present) bypasses project
discovery. Every other command — including agent-mode launch-bundle — requires
`mars.toml` in an ancestor directory.

### Warning accumulation pipeline

```
parse_diags           ← frontmatter parse warnings (agent mode only)
policy.warnings       ← harness resolution issues, model resolution issues,
                        linked-constraint degradation, experimental harness
prompt.warnings       ← missing skills
tool_warnings         ← unknown tool names on first-class harnesses
routing.warnings      ← always empty (see runnable.rs contract)
```

All warning vectors are `extend()`-ed into a single `LaunchBundle.warnings` field.

## Architecture

```
LaunchBundleRequest {agent, model, harness, effort, approval, sandbox, extra_skills}
    │
    ├─ [agent mode] read + parse .mars/agents/<name>.md → AgentProfile + body
    ├─ [ad-hoc mode] empty_agent_profile() + "" body
    │
    └─ resolve_policy()
           ├─ config::load_policy_resolution_config()  (aliases, harness_order, linked targets)
           ├─ model::resolve_model()                   (alias → model_id + provider)
           ├─ harness::resolve_harness()               (route selection, provider/candidate eval)
           ├─ execution::resolve_execution_policy()    (effort, approval, sandbox, autocompact)
           └─ runnable::resolve_routing()              (populate Routing, warnings always empty)
       │
       ├─ resolve_effective_skills()  (profile skills filtered by harness kind)
       ├─ compile_prompt_surface()    (system instruction + supplemental docs + inventory)
       └─ resolve_bundle_tools()      (tool normalization per harness)
           │
           └─ LaunchBundle {routing, execution_policy, prompt_surface, tools, skills_metadata, provenance, warnings}
```

`MarsContext` is the single object passed through; it carries `project_root` and
`managed_root`. For ad-hoc mode with a synthetic context, `managed_root` resolves to
`<cwd>/.mars` which may not exist — but `resolve_policy` handles missing config
gracefully (empty aliases, empty model cache).

## Rationale

Ad-hoc mode exists because callers outside a Mars-managed project (e.g., Meridian CLI,
scripting) need routing decisions without a full `mars sync` pipeline. The guard in
`can_run_without_project` scopes this bypass narrowly — only `--model` + no `--agent`
ad-hoc launch-bundles skip project discovery.

Warning semantics are split between `warnings` (user-actionable) and routing/provenance
fields (route metadata) because callers consume the bundle differently. A downstream
harness adapter reads `routing.harness_model` to know which model ID to pass to the
provider; it doesn't need to see `"passthrough"` as a warning. But a user configuring
`settings.targets = [".codex"]` needs to know that linked-harness constraints blocked
normal routing. Confusing these two categories produces noise that desensitizes users
to real warnings.

### Anti-patterns for this module

- Do NOT add warnings for harness-model path facts (`passthrough`, `synthesized`,
  `unknown` confidence). These belong in routing fields.
- Do NOT call `build_launch_bundle` with an agent name from outside a synced project —
  the agent file must exist at `.mars/agents/<name>.md`.
- Do NOT emit `eprintln!` in library code — all diagnostics go through the warnings
  vector and are surfaced by the CLI layer.

## Related docs

- [src/routing/.context/CONTEXT.md](../routing/.context/CONTEXT.md) — harness candidate
  evaluation, RouteConfidence semantics, link filtering
- [src/harness/.context/CONTEXT.md](../harness/.context/CONTEXT.md) — harness registry,
  capability snapshot, probe integration
- [src/cli/build.rs](../cli/build.rs) — CLI arg definitions and entry points
- [tests/smoke/manual/results-launch-bundle-resolver.md](../../tests/smoke/manual/results-launch-bundle-resolver.md) —
  smoke evidence for routing and warning behavior
