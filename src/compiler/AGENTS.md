# src/compiler/ — Package Compilation Pipeline

Compiles resolved packages into target state. 9 submodules, ~6900 lines.

## Mental Model

```
ReaderIr (source-level facts)
    ↓
build_target()     ← assign dest paths, handle collisions, rewrite frontmatter refs
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

## Dual-Surface Compilation

Under `EmitAll` policy (standalone mars), harness-bound agents are compiled to both:
1. `.mars/agents/` — canonical universal format
2. `<target>/agents/` — harness-native format (e.g., `.claude/agents/coder.md`)

Under `SuppressAll` (Meridian-managed), native artifacts are removed.

### Agent Surface Policy

| Setting | Meridian-managed | Policy |
|---|---|---|
| unset (auto) | false | EmitAll |
| unset (auto) | true | SuppressAll |
| always | any | EmitAll |
| never | any | SuppressAll |

## Agent Compilation (`agents/`)

- `mod.rs` — schema parser, `AgentProfile` from YAML frontmatter + markdown body
- `lower.rs` — per-harness lowering with lossiness tracking
- `HarnessKind` — Claude, Codex, OpenCode, Cursor, Pi

### Non-Overridable Fields

These fields cannot appear inside `harness-overrides` blocks: `name`, `description`, `model`, `harness`, `mode`, `model-invocable`, `model-overrides`, `harness-overrides`.

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

**Test dual-surface:**
```rust
let policy = agent_surface_policy(None, false); // standalone → EmitAll
reconcile_native_agent_surfaces(policy, project_root, mars_dir, &outcomes, false, &mut diag);
```

**Test suppression:**
```rust
let policy = agent_surface_policy(Some(&AgentEmission::Auto), true); // meridian → SuppressAll
```

## Linked-target writes

Native reconcile and dual-surface compile gate deletes and copies through `surface_ownership` (same rules as `target_sync`). See `src/target_sync/.context/CONTEXT.md`.

## See Also

- `src/sync/AGENTS.md` — orchestrates the compiler
- `src/target/AGENTS.md` — per-target adapters the compiler uses
- `src/target_sync/.context/CONTEXT.md` — per-target lock ownership and collision semantics
- `src/compiler/agents/mod.rs` — AgentProfile schema details
