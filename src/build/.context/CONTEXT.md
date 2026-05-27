# src/build/

Generates launch bundles — the serializable artifact that a harness runtime consumes to
launch an agent with resolved routing, execution policy, tools, and prompt surface.

## Contracts

### Two launch-bundle modes

1. **Ad-hoc mode** (no `--agent`):
   - `--model` is optional; when omitted, routing selects an installed/default
     harness and leaves `routing.model`, `routing.model_token`, and
     `routing.harness_model` empty so the harness can use its own default model
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

### Cross-field precedence soft-fail

When a fixed harness conflicts with a model from a lower-precedence source, the harness wins
and the model is cleared. Precedence ranks (`PolicySource::precedence_rank()`):

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
`pi_incompatible`) stay hard errors regardless of precedence rank — the model field being cleared
cannot resolve a provider constraint or compatibility failure.

**Same-precedence conflicts are hard errors.** CLI harness + CLI model both `no_model_match` →
error; the user explicitly requested an impossible combination.

`HarnessResolution.model_override: Option<()>` signals the outcome. When `Some(())`, `policy/mod.rs`
substitutes empty strings for `model`, `model_token`, `provider_constraint`, and `provider_for_order`
before calling `runnable::resolve_routing` — the harness uses its own default model. A warning
(`"<source> model '<token>' cannot run on <source> harness '<name>'; clearing model"`) flows
through the normal warnings pipeline to the bundle's `warnings[]` field.

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
                    ..
                })
            }),
            MarsError::Config(ConfigError::ProjectRootNotFound { .. })
        )
    )
}
```

Only ad-hoc launch-bundle (`--agent` absent) bypasses project discovery. Every
other command — including agent-mode launch-bundle — requires `mars.toml` in an
ancestor directory.

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

### Catalog refresh (`ensure_fresh`)

`resolve_policy` calls `models::ensure_fresh` on `.mars/` before harness evaluation — not a
read-only cache read. TTL and stale fallback follow [`src/models/AGENTS.md`](../../models/AGENTS.md).
`LaunchBundleRequest.models_refresh` carries `ModelsRefreshControl` from CLI
(`--refresh-models` → force sync catalog + synchronous probes;
`--no-refresh-models` → offline catalog + skip probe refresh; default → auto/background).

Catalog slugs feed `RoutingInput.catalog_model_slugs` so native harness matching aligns with
`mars models list|resolve` (same `evaluate_candidates` path).

### `harness_model` vs `candidate_slugs`

`LaunchBundle.routing.harness_model` is the **runtime model id** for the selected harness
(including Cursor effort baking in `runnable.rs`). `routing.candidate_slugs` copies assessment
probe/catalog candidates for debugging only — bundle doc comment: consumers run `harness_model`
verbatim. `routing.candidate_slugs` on the trace/report is diagnostic; do not use it for launch.

### Alias `provider` → `harness_model`

Resolved in `models::resolve_harness_model` ([`src/models/.context/CONTEXT.md`](../../models/.context/CONTEXT.md)):

- **Codex / Claude:** when alias or routing `provider` matches the native harness, emit the
  **bare** canonical model id (`gpt-5.4-mini`, not `openai/gpt-5.4-mini`).
- **Pi / OpenCode:** pick a probe-listed slug (`openai-codex/gpt-5.4-mini`, etc.); use
  `provider_constraint` only to order/filter slugs, not to prefix before the probe runs.

`harness_model_source` / `harness_model_confidence` record how the id was chosen (`provider-match`,
`cached-probe`, `passthrough`) — still not user-facing warnings.

### Cursor effort → `harness_model`

When harness is `cursor` and effort is set, `resolve_routing` calls
`resolve_cursor_effort_slug` against probe slugs. Default-tier efforts (`medium`, `none`,
`auto`, `default`) select the unsuffixed base slug when present; other tiers use suffixed
slugs. Success sets `harness_model_source` / `confidence` to cached-probe confirmed and
marks effort consumed (cleared from execution policy output).

## Architecture

`build_launch_bundle()` resolves project config once, then passes it into policy resolution.

```
LaunchBundleRequest {agent, model, harness, effort, approval, sandbox, extra_skills}
    │
    ├─ [agent mode] read + parse .mars/agents/<name>.md → AgentProfile + body
    ├─ [ad-hoc mode] empty_agent_profile() + "" body
    │
    ├─ load_effective_project_config_or_default(project_root)
    │      └─ default only when mars.toml is absent
    │
    └─ resolve_policy(&EffectiveProjectConfig, PolicyInput)
           ├─ models::merged_runtime_aliases()         (consumer + dependency aliases)
           ├─ models::ensure_fresh()                   (catalog for native slug + probes)
           ├─ model::resolve_model()                   (alias → model_id + provider, or unset)
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
`can_run_without_project` scopes this bypass narrowly — only no-`--agent`
ad-hoc launch-bundles skip project discovery, with `--model` optional.

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

- [src/models/AGENTS.md](../../models/AGENTS.md) — catalog `ensure_fresh`, `ModelsRefreshControl`
- [src/routing/.context/CONTEXT.md](../../routing/.context/CONTEXT.md) — harness candidate
  evaluation, selection-kind vs match-evidence semantics, and `RouteDecisionReport`
  serialization surface
- [src/harness/.context/CONTEXT.md](../../harness/.context/CONTEXT.md) — harness registry,
  capability snapshot, probe integration
- [src/cli/build.rs](../../cli/build.rs) — CLI arg definitions and entry points
- [tests/smoke/manual/results-launch-bundle-resolver.md](../../../tests/smoke/manual/results-launch-bundle-resolver.md) —
  smoke evidence for routing and warning behavior
