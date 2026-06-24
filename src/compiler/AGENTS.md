# src/compiler/ — Package Compilation Pipeline

Compiles resolved packages into target state. 11 submodules, ~2100 lines in compiler
core (`mod.rs`, `native_agents.rs`) plus per-lane modules (agents, skills, config
entries, hooks, MCP, variants, visibility).

## Mental Model

```
ReaderIr (source-level facts)
    ↓
build_target()     ← assign dest paths, reject cross-source collisions, rewrite frontmatter refs
    ↓
create_plan()      ← diff + plan
    ↓
apply_plan()       ← persist config, write to .mars/ canonical store
    ↓
dual_surface_compile()  ← emit native harness artifacts (.claude/agents/, etc.)
config_entries_compile() ← MCP servers, hooks
    ↓
sync_targets()     ← copy to managed target directories
    ↓
finalize()         ← write lock, build report
```

The compiler is the second half of the sync pipeline. It consumes `ReaderIr` and produces `SyncReport`.

## Module Map

- `mod.rs` (304 lines) — orchestration: `compile()` entry point, stages, lock finalization
- `native_agents.rs` (814 lines) — native harness surface lifecycle: scan, reconcile, compile,
  emit, link-materialize. Extracted from `mod.rs` (was 1522 lines). Coexists with `native_agents/tests.rs`.
- `native_agent_manifest.rs` — lock-then-native-agent-manifest persistence
- `agent_copy.rs` — validates `settings.meridian.agent_copy` into an emission allowlist
- `tool_policy.rs` — shared tool-policy parsing (`tools:` list-or-map, `disallowed-tools:`, inline `mcp(...)` grants) into `EffectiveToolPolicy`. Used by **both** `agents/` and `skills/` modules for unified frontmatter-to-native lowering.
- `lossiness.rs` — lossiness classification types (`LossyField`, `Lossiness`, `LoweredOutput`) and grouped diagnostic emission (`emit_agent_lossiness_warnings`, `emit_skill_lossiness_warnings`)
- `lossiness_preview.rs` — preview lossiness diagnostics without running sync (`collect_source_lossiness_diagnostics`). Called by `mars check` and `mars init`.
- `invocability.rs` — shared `model-invocable` / `user-invocable` axis parsing for both agents and skills
- `tool_names.rs` — canonical snake_case tool name recognition + per-target projection (PascalCase for Claude/Cursor/Pi, snake_case for Codex, lowercase-without-underscores for OpenCode)
- `agents/` — `AgentProfile` schema parser + per-harness lowering with model alias resolution
- `skills/` — universal skill schema + native lowering with variant layouts
- `config_entries/` — MCP servers and hooks from packages → target config files
- `mcp/` — MCP server item discovery, env-ref validation, collision detection
- `hooks/` — hook item discovery, event validation, ordering, lossiness classification
- `variants/` — skill variant layout validation, indexing, projection
- `visibility/` — D1/D10 propagation rules

## Dual-Surface Compilation

Under `EmitAll` policy (standalone mars), **every source agent** is compiled to **every configured managed target harness** (from `settings.targets`):
1. `.mars/agents/` — canonical universal format
2. `<target>/agents/` — harness-native format (e.g., `.claude/agents/coder.md`); model set to `Clear` when the agent's model does not resolve to that harness

An agent with no matching model is still emitted — its native file omits the model field so the harness uses its own default. Profile `harness:` is a model-selection hint, not an emission filter.

Under `EmitSelective` (via `settings.meridian.agent_copy`), only agents whose model resolves to the configured harnesses are emitted.

Under `SuppressAll` (Meridian-managed, or `agent_emission = "never"` without agent_copy), native artifacts are removed.

### Model Alias Resolution at Compile Time

When lowering a universal agent profile to a native harness format, model aliases like
`opus46` are resolved to concrete model IDs (e.g., `claude-opus-4-6`) at compile time.
This is necessary because native harnesses (Claude Code, Codex) don't understand the
alias system. `NativeModelRoutingRuntime` in `native_agents.rs` owns the merged alias
registry, `.mars/models-cache.json`, catalog slugs, routing settings, one lazy
capability session, and memoized `(model token, target harness)` decisions. It resolves
profile `model` first, then model-policy candidates when fanout is enabled, and delegates
accept/reject to `routing::evaluate_candidates*` constrained to the target harness.

### Agent Surface Policy

| Setting | Meridian-managed | agent_copy | Policy |
|---|---|---|---|
| unset (auto) | false | any | EmitAll |
| unset (auto) | true | none | SuppressAll |
| unset (auto) | true | configured | EmitSelective |
| always | any | any | EmitAll |
| never | any | none | SuppressAll |
| never | any | configured | EmitSelective |

## Agent Compilation (`agents/`)

- `mod.rs` — schema parser, `AgentProfile` from YAML frontmatter + markdown body
- `lower.rs` — per-harness lowering with lossiness tracking. All lowerers accept a
  `model_field: &NativeModel` parameter (`Set(id)` or `Clear` for native compile;
  `Inherit` remains for direct lowerer tests). `lower_for_harness_with_model()`
  dispatches to the correct lowerer, ensuring emitted native artifacts carry concrete
  model IDs rather than aliases.
- `HarnessKind` — Claude, Codex, OpenCode, Cursor, Pi

### Harness Override Passthrough

`harness-overrides.<harness>` blocks are target-native passthrough. Mars validates only the outer mapping shape and serializability; nested keys do not replace top-level Mars semantic fields. Unknown override harness keys are warnings and are preserved for forward compatibility.

## Shared Tool Policy (`tool_policy.rs`)

`EffectiveToolPolicy` is the unified tool-gating schema used by **both** `agents/` and `skills/`. It merges three canonical fields from frontmatter:

| Field | Form | Merged into |
|---|---|---|
| `tools:` | List (all allowed) or map (`tool_name: allow\|deny`) | `allowed` / `disallowed` |
| `disallowed-tools:` | Flat string list | `disallowed` |
| `mcp(...)` in `tools:` / `disallowed-tools:` | Inline MCP grant/deny | `mcp_allowed` / `mcp_disallowed` |

`agents/mod.rs::AgentProfile::effective_tool_policy()` and `skills/mod.rs::SkillProfile::effective_tool_policy()` both delegate to `tool_policy::effective_tool_policy()` with field-level dedup and canonical name normalization.

MCP grammar and per-harness projection: `.context/CONTEXT.md`.

## Tool Name Projection

Mars canonical tool names are snake_case. `tool_names.rs` recognizes canonical names plus aliases/native spellings, then projects by target convention: Claude/Cursor/Pi PascalCase, Codex snake_case, OpenCode lowercase-without-underscores. Only true semantic exceptions live in the override table (for example Codex `bash` → `shell`, OpenCode `read` → `view`). Harness support for a specific known tool is separate from name recognition.

## Skill Compilation (`skills/`)

Universal schema parsing and native lowering. Skills support variant layouts per harness target.

## Config Entries (`config_entries/`)

MCP servers and hooks discovered from packages, validated, and written to target config files via adapters.

## MCP Compilation (`mcp/`)

Discovers MCP server items, validates env refs, detects collisions.

## Hooks Compilation (`hooks/`)

Discovers hook items, validates events, orders bindings, classifies lossiness.

## Variants (`variants/`)

Skill variant layout validation, indexing, and projection per harness target.

## Visibility (`visibility/`)

Propagation rules for passive vs effectful items (D1/D10).

## Patterns

**Policy selection:**
```rust
// standalone → EmitAll
let policy = agent_surface_policy(None, None, false);
// meridian-managed → SuppressAll
let policy = agent_surface_policy(Some(&AgentEmission::Auto), None, true);
// selective emission via settings.meridian.agent_copy
let policy = agent_surface_policy(Some(&AgentEmission::Auto), spec.as_ref(), true);
```

**Reconcile:**
```rust
let ctx = NativeAgentReconcileCtx {
    policy, project_root, model_aliases, outcomes, old_lock, dry_run, selective_harness_scope,
};
let removed = reconcile_native_agent_surfaces(&ctx, &mars_agents, diag);
```

**Compile (EmitAll → every agent to every configured managed target):**
```rust
let ctx = NativeAgentCompileCtx {
    project_root, model_aliases, cursor_probe_slugs, old_lock, harness_scope,
    configured_emit_harnesses, options,
};
let outputs = compile_native_agents(&ctx, &AgentSurfacePolicy::EmitAll, &mars_agents, diag);
```

## Linked-target writes

Native reconcile and dual-surface compile gate deletes and copies through `surface_ownership` (same rules as `target_sync`). See `src/target_sync/.context/CONTEXT.md`.

## See Also

- `src/sync/AGENTS.md` — orchestrates the compiler
- `src/target/AGENTS.md` — per-target adapters the compiler uses
- `src/target_sync/.context/CONTEXT.md` — per-target lock ownership and collision semantics
- `src/compiler/agents/mod.rs` — AgentProfile schema details
