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
- `tool_policy.rs` — runtime projection, overlay merging and specificity over schema-owned tool rules. Authored parsing, vocabulary, aliases and native tables live in `mars-schema`.
- `lossiness.rs` — lossiness classification types (`LossyField`, `Lossiness`, `LoweredOutput`) and grouped diagnostic emission (`emit_agent_lossiness_warnings`, `emit_skill_lossiness_warnings`)
- `lossiness_preview.rs` — preview lossiness diagnostics without running sync (`collect_prepared_source_lossiness_diagnostics`). Called by `mars check` and `mars init`.
- `mars-schema::invocability` — shared `model-invocable` / `user-invocable` axis parsing for both agents and skills
- `mars-schema::tool_names` — the capability vocabulary, aliases and `tool(pattern)` grammar
- `mars-schema::tool_natives` — explicit per-harness native tool tables. Nothing is derived by naming convention.
- `harness_descriptor.rs` — typed compiler harness facts, including canonical and display names,
  target roots, variant keys, and lowering-policy selectors
- `agents/` — `AgentProfile` schema parser + per-harness lowering with model alias resolution
- `skills/` — universal skill schema + native lowering with variant layouts
- `config_entries/` — selected MCP servers and hooks → target config files
- `mcp/` — MCP server item discovery, env-ref validation, collision detection
- `hooks/` — fragment-manifest discovery, merge-fragment validation/substitution, and ordering
- `variants/` — skill variant layout validation, indexing, projection
- `visibility/` — typed `local` / `exported` parsing for effectful items

## Dual-Surface Compilation

Under `EmitAll` policy (standalone mars), **every source agent** is compiled to **every configured managed target harness** (from `settings.targets`):
1. `.mars/agents/` — canonical universal format
2. `<target>/agents/` — harness-native format (e.g., `.claude/agents/coder.md`); model set to `Clear` when the agent's model does not resolve to that harness

An agent with no matching model is still emitted — its native file omits the model field so the harness uses its own default. Profile `harness:` is a model-selection hint, not an emission filter.

Under `EmitSelective` (via `settings.meridian.agent_copy`), only agents whose model resolves to the configured harnesses are emitted.

Under `SuppressAll` (Meridian-managed, or `agent_emission = "never"` without agent_copy), native artifacts are removed.

### Model Alias Resolution at Compile Time

When lowering a universal agent profile to a native harness format, model aliases like
`sol` are resolved to concrete model IDs (e.g., `gpt-5.6-sol`) at compile time.
This is necessary because native harnesses (Claude Code, Codex) don't understand the
alias system. `NativeModelRoutingRuntime` in `native_agents.rs` owns the merged alias
registry, `.mars/models-cache.json`, catalog slugs, routing settings, one lazy
capability session, and memoized `(model token, target harness)` decisions. It resolves
profile `model` first, then model-policy candidates when consumer fanout is enabled, and delegates
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

- `mars-schema::agent` — the schema-v1 reader and `AgentProfile` authority
- `lower.rs` — per-harness lowering with lossiness tracking. All lowerers accept a
  `model_field: &NativeModel` parameter (`Set(id)` or `Clear` for native compile;
  `Inherit` remains for direct lowerer tests). `lower_for_harness_with_model()`
  dispatches to the correct lowerer, ensuring emitted native artifacts carry concrete
  model IDs rather than aliases.
- `lower_policy.rs` owns capability-aware agent skill lowering. Claude and Cursor native
  `skills:` fields receive `skills.load` only; body guidance carries `skills.available`, plus
  `skills.load` on targets without per-agent preload and on Claude's dual-use
  subagent/session-agent file. This guidance is a designed mapping, not warning-tier lossiness.
  Plugin builds pass qualified skill names into the same lowering path; their prose rewriter
  handles authored references separately.
- `mars_schema::HarnessId` — Claude, Codex, OpenCode, Cursor, Pi
- `network`, `network-allow`, and `network-deny` are launch-time Meridian fields. Native
  lowerers omit them; Codex records all three as dropped, while other targets record them as
  Meridian-only. They are read from the profile as it launches
  (`build::policy::profile_for_model`), so an overlay, a model-policy `network` override and
  the `[settings]` defaults count.
### Harness Override Passthrough

`harness-overrides.<harness>` blocks are target-native passthrough on Agents and skills. Mars validates only the outer mapping shape and serializability. Lowering writes each harness's own block into its native file (Markdown frontmatter; extra top-level keys after Codex's) and never another harness's; a key the lowering owns (`mars_schema::agent::LoweredField::keys`, `mars_schema::skill::LOWERED_KEYS`, Codex's seven) keeps Mars's value and the override is reported as `Dropped`. OpenCode's `permission` is the exception: Mars writes `deny` entries for the Agent's deny rules and merges them over the author's `harness-overrides.opencode.permission`, so only an entry that contradicts a deny is dropped. Unknown (non-override) keys reach no native file. Unknown override harness keys are warnings and are preserved for forward compatibility.

## Shared Tool Policy (`tool_policy.rs`)

`EffectiveToolPolicy` is the one tool-gating model for **agents** and launch bundles. The
schema reader accepts only the v1 action-map form and stores
`ToolRule { capability, pattern, action }` values. Consumer `[agents.*.tools]` overlays
edit that rule list; skills do not carry tool policy in schema v1.

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

MCP servers and hooks are selected during the target build, then validated and written to target
config files via adapters. `TargetState` carries each selected parsed item and its managed targets,
plus filtered exported graph hooks for plugin omission diagnostics; preflight and emission consume
the selected values instead of rediscovering package config entries. Sync checks a selected hook's
installed directory after target sync, while a plugin build can lower the same pure selection
without an installed directory.
`lower_config_entries_for_target` owns per-target MCP collision resolution, native merge-fragment
loading, and hook ordering. Callers supply the `${MARS_HOOK_DIR}` replacement through its
hook-directory callback.

## MCP Compilation (`mcp/`)

Discovers MCP server items, validates env refs, detects collisions.

## Hooks Compilation (`hooks/`)

Fragment-manifest discovery, validation, and loading. Discovers `hook.toml` items,
validates merge-mode event keys against per-target allowlists, unwraps copy-paste
wrappers, substitutes `${MARS_HOOK_DIR}` in all JSON string values and file-mode
fragment text, and returns structured `HookFragment` or substituted file content.
Placement and lock recording are handled by `config_entries/`.
Dependency hook discovery is contextualized by `hooks::discover_resolved_hook_items` during target
construction; callers must not expose derived staging paths in schema errors. Target construction
derives the filter-facing `DiscoveredItem` from that strict parse instead of parsing `hook.toml`
again.

## Variants (`variants/`)

Skill variant layout validation, indexing, and projection per harness target.

## Visibility (`visibility/`)

Hook and MCP manifests parse visibility once into `Visibility`. Unknown values warn and become
`Local`; downstream selection only compares the enum with `Exported`.

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
    project_root, old_lock, dry_run, selective_harness_scope,
};
let removed = reconcile_native_agent_surfaces(&ctx, &desired_outputs, diag);
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
Reconcile compares every prior native-agent lock claim in scope with the exact output set qualified
for this run. It therefore removes old names and outputs belonging to dependencies that disappeared
or stopped qualifying, rather than inferring cleanup from the current canonical agent list.
File-mode hook emission also reserves its exact `OutputRecord` against the
post-sync ownership lock before writing. Lock finalization retains every prior
linked output unless its removal was positively confirmed, so failed filesystem
removals remain retryable.

## See Also

- `src/sync/AGENTS.md` — orchestrates the compiler
- `src/target/AGENTS.md` — per-target adapters the compiler uses
- `src/target_sync/.context/CONTEXT.md` — per-target lock ownership and collision semantics
- `src/compiler/agents/mod.rs` — AgentProfile schema details
