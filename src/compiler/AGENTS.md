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
sync_targets()     ← copy canonical content to managed target directories
    ↓
config_entries_compile() ← emit MCP servers and bindings for installed hooks
dual_surface_compile()   ← emit native harness artifacts (.claude/agents/, etc.)
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
- `tool_policy.rs` — tool-policy parsing (`tools:` list-or-map, `disallowed-tools:`, inline `mcp(...)` grants) into one typed `EffectiveToolPolicy` of `ToolRule`s, the `mars.toml` overlay edit, and `EffectiveToolPolicy::project`, the one place a rule becomes harness-native names. Used by `agents/`, `skills/` and the launch bundle (`build/`).
- `lossiness.rs` — lossiness classification types (`LossyField`, `Lossiness`, `LoweredOutput`) and grouped diagnostic emission (`emit_agent_lossiness_warnings`, `emit_skill_lossiness_warnings`)
- `lossiness_preview.rs` — preview lossiness diagnostics without running sync (`collect_source_lossiness_diagnostics`). Called by `mars check` and `mars init`.
- `invocability.rs` — shared `model-invocable` / `user-invocable` axis parsing for both agents and skills
- `tool_names.rs` — the capability vocabulary (canonical names + aliases), the `Capability` type and the `tool(pattern)` grammar
- `tool_natives.rs` — explicit per-harness native tool tables, pinned by a test against each harness's real tool list. Nothing is derived by naming convention.
- `agents/` — `AgentProfile` schema parser + per-harness lowering with model alias resolution
- `skills/` — universal skill schema + native lowering with variant layouts
- `config_entries/` — MCP servers and hooks from packages → target config files
- `mcp/` — MCP server item discovery, env-ref validation, collision detection
- `hooks/` — fragment-manifest discovery, merge-fragment validation/substitution, and ordering
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
- `network`, `network-allow`, and `network-deny` are launch-time Meridian fields. Native
  lowerers omit them; Codex records all three as dropped, while other targets record them as
  Meridian-only.

### Harness Override Passthrough

`harness-overrides.<harness>` blocks are target-native passthrough. Mars validates only the outer mapping shape and serializability; nested keys do not replace top-level Mars semantic fields. Unknown override harness keys are warnings and are preserved for forward compatibility.

## Shared Tool Policy (`tool_policy.rs`)

`EffectiveToolPolicy` is the one tool-gating model of **agents**, **skills** and the **launch
bundle**: a list of `ToolRule { capability, pattern, action }`, parsed once into
`AgentProfile.tools` / `SkillProfile.tools` by `parse_tool_policy`. `capability` is a
`Capability` (`Any` for `*`, `Mcp(McpRef)`, a vocabulary `Builtin`, or the author's `Unknown`
spelling), never a string with reserved values.

- **Parse.** Deprecated `tools:` list → `allow` per entry and a warning; map → `allow` /
  `deny` / `ask` per entry; `disallowed-tools:` → `deny`. Lists only grant for agents and
  skills. An allowlist is a map with an explicit `'*': deny`. A `bash` pattern `X:*` becomes
  `X *`.
- **Overlay.** A `mars.toml` `[agents.x.tools]` / `[skills.x.tools]` overlay is
  `EffectiveToolPolicy::apply_overlay`: one edit of the rule list for both. An agent's native
  lowering and the bundle call it on the parsed profile; a skill's staging calls it and writes
  the rules back, since sync lowers skills from the staged file. Plain permission lists add;
  `{ replace = [...] }` clears lower rules for that action first. Ask rules and an explicit
  `'*': deny` allowlist marker survive either form. Overlay names are `OverlayToolName`s: parsed
  with the profile grammar as `mars.toml` loads, so `apply_overlay`
  never meets a name a profile would reject. Staging writes a skill back only when its own
  `tools:` / `disallowed-tools:` parsed without a diagnostic: rewriting from a lossy parse would
  drop the bad entry, a malformed deny among them, and hide the error sync reports for it.
- **Project.** `EffectiveToolPolicy::project(harness)` is the only path from a rule to native
  permission names and unscoped harness tool names. Native agent lowering, skill lowering and the
  bundle read its `natives` / `gaps` / `overlaps`; never add a second projection path, or a
  consumer that re-derives either fact. A projected rule keeps permission→tool bindings so
  conflict resolution removes `native_tools` with the `native` permission it removes.
  The bundle's profile-independent `agent` / `task` capability metadata calls
  `project_platform_capability`, which shares the builtin row translation with rule projection
  rather than interpreting `tool_natives` itself.
  A rule's `NativeStatus::Unknown(reason)` covers every permission gap: a harness with no
  switch for the tool (a `Native::NoSwitch` row), no row at all, an unrecognized name, an MCP
  ref the harness cannot represent. The bundle prints it as `unknown` with the reason as
  `note` and adds no `warnings[]` entry: Meridian prints those at every launch, so a gap is
  report-only there. `mars sync` is where an author hears about a typo or an MCP ref.
  A known rule can also carry a `note` when the valid harness permission has broader effects.
  Resolving a narrow rule over a broad one (`resolve_by_specificity`) happens inside it; when
  scoped Claude capabilities collapse to the same `Edit(path)` or `Read(path)` permission,
  Claude's documented `deny` → `ask` → `allow` order leaves only the strictest action.

What the bundle's `tools.rules` mean and what a status says: `docs/cli/commands.md`.
MCP grammar and per-harness projection: `.context/CONTEXT.md`.

## Tool Name Projection

Mars owns native tool names. `tool_names.rs` recognizes a capability from canonical names,
aliases and native spellings (`Bash`, `shell`, `AskUserQuestion` → `bash`, `ask_user`). The
vocabulary is deliberate: `task` is the task list, `agent` is sub-agents, `web` is
`web_search` + `web_fetch`. `tool_natives.rs` has one table per harness, one row per
capability, each one-to-many (Claude bare `edit` → `Edit`, `Write`, `NotebookEdit`) with an
explicit scoped spelling where the permission engine differs (Claude scoped file edits use
`Edit(path)`, reads use `Read(path)`). The unscoped set is also the authoritative
`native_tools` availability list, even when a scoped permission uses a category name. Each row
also says why an unknown is unknown (`Absent`, `NoSwitch`, or no row yet). Nothing is derived
by naming convention. Add a tool by adding it to the harness's real-tool list in the test
first, with its source.

## Skill Compilation (`skills/`)

Universal schema parsing and native lowering. Skills support variant layouts per harness target.

## Config Entries (`config_entries/`)

MCP servers and hooks discovered from packages, validated, and written to target config files via adapters.

## MCP Compilation (`mcp/`)

Discovers MCP server items, validates env refs, detects collisions.

## Hooks Compilation (`hooks/`)

Fragment-manifest discovery, validation, and loading. Discovers `hook.toml` items,
validates merge-mode event keys against per-target allowlists, unwraps copy-paste
wrappers, substitutes `${MARS_HOOK_DIR}` in all JSON string values and file-mode
fragment text, and returns structured `HookFragment` or substituted file content.
Placement and lock recording are handled by `config_entries/`.
Dependency hook discovery is contextualized by
`hooks::discover_resolved_hook_items` in both target construction and config
preflight; callers must not expose derived staging paths in schema errors.

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
File-mode hook emission also reserves its exact `OutputRecord` against the
post-sync ownership lock before writing. Lock finalization retains every prior
linked output unless its removal was positively confirmed, so failed filesystem
removals remain retryable.

## See Also

- `src/sync/AGENTS.md` — orchestrates the compiler
- `src/target/AGENTS.md` — per-target adapters the compiler uses
- `src/target_sync/.context/CONTEXT.md` — per-target lock ownership and collision semantics
- `src/compiler/agents/mod.rs` — AgentProfile schema details
