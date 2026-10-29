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
- `hooks/` — hook discovery, fragment reading, and the location added to `mars-schema::hooks` diagnostics; the model and every hook rule are `mars-schema::hooks`
- `variants/` — skill variant layout validation, indexing, projection

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
  (`build::policy::profile_for_model`), so an overlay and a model-policy `network` override
  count. The `[settings]` network does not: it is the project's, and a plugin build never
  receives the package's own.
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
config files via adapters. `TargetState` carries one parsed selection per target, plus filtered
accepted hooks (every hook past the package filters and visibility, local ones too) for the skip report and plugin omission diagnostics; preflight and emission consume the selected
values instead of rediscovering package config entries. After target sync, sync withholds a hook's
or a token-naming MCP server's entry unless target sync recorded installing that directory, so an
unmanaged or user-edited copy keeps the entry out with a `hook-not-installed` / `mcp-not-installed`
warning naming the path; target sync's `target-unmanaged-collision` or `target-divergent` warning
says why. A token-free server has no directory to check. `config_entries/render.rs`
preflight reads, validates and renders every selected hook once (`RenderedHooks`); compile reuses
that result, and `lower_merge_fragments` orders each hook's merge output, kept hooks' recorded entries included (a kept hook carries its `HookRank` from the sync that found it invalid: merge order is the only source of order in a target's file) (`lower_hook_entries` is
the same for a plugin build, which has no preflight; there `${MARS_HOOK_TARGET}` is `plugin`, and the Claude tree's output goes through `HookDef::plugin_output`, which holds it to what Codex parses because Codex reads that file too, D21).
Preflight also says what `[hooks]` leaves out on every managed target, for every accepted hook even when no target takes it (`hook-event-skipped`:
a warning per skip for the project's own hooks, one info line per sync for dependencies) and
a fragment replacing generated events (`hook-fragment-overrides`), a file-target fragment beside the generated `mars-<name>.hooks.ts` (`hook-fragment-beside-generated`) and answers a TypeScript target cannot carry (`hook-answer-unsupported`, info). After the writes, compile
compares a trust-gated target's (event, group, handler) positions with those before the sync and
warns `codex-hooks-need-trust` for any new or moved one. `config_entries/file_hooks.rs` owns file-placement
outputs (a file whose bytes already equal the output is not rewritten): what the old lock says a hook owns on a target (every non-canonical output but its
directory copy, so a hook may own several files) and writing them. `lower_mcp_servers` lowers servers.
A target whose merge-file hook entries equal the lock's records, and whose file still holds
them in the order a sync would write them (`hook_file::holds_recorded`, matched entry by entry),
is not rewritten (Codex keys trust on entry position). A changed `order` therefore rewrites the
file at once, moving the entries and raising `codex-hooks-need-trust`; a sync that changed nothing
stays silent.

A dependency hook whose `hook.toml` fails validation (D19) is `KeptHook` in `TargetState`:
`sync::diff` keeps its `.mars` directories as the lock has them, compile carries its merge
records (re-appended from the record if the file is rewritten for another reason) and file
outputs, and target sync re-projects the unchanged directory as for any unchanged item. A hook
never installed installs nothing; both warn `hook-invalid-kept`. A dependency hook is judged
invalid in one place, discovery: its fragments must also load (`HookDef::check_fragment`) on each
managed target it is installed for; `[hooks]` is rendered once, at preflight. The project's own hook that fails to render is a hard error at preflight.
The hook kept is whatever the old lock installed from that package under the `name` its broken
file still declares, else its directory's name (the lock records no source path). A hook the
consumer excludes, or whose file does not still say `visibility = "exported"`, keeps nothing and
is not reported. A dependency hook in the
removed schema, and any invalid project hook, still fail the sync. A dependency's unknown
top-level key and unknown merge-fragment event warn (`Origin::Dependency`) and install. MCP collisions resolve once per target in
`TargetState`, so the winning server's directory and config entry come from the same selection.
Each selected item arrives with its directory token, the `${MARS_HOOK_DIR}` /
`${MARS_MCP_DIR}` replacement, so installed copies and plugin trees share one lowering path.
Both tokens render absolute: a relative path would break in a git worktree, where a committed
`.mcp.json` points at a server directory that only the main checkout has.

## MCP Compilation (`mcp/`)

Discovers MCP server items, validates env refs (stdio `env` and remote header refs), and
substitutes `${MARS_MCP_DIR}` in a stdio server's `command`/`args`. `mars-schema::mcp_server`
owns the authored definition (`McpServerDef`, `McpTransport::{Stdio, Http}`, `HeaderValue`,
`EnvRef`) and the `mcp.toml` parse; this lane consumes those types and never re-parses. A
remote server names no directory, so the token in its `url` or headers is an error. Only a server whose command or args name the token installs its
directory, as a package-owned item beside each target that writes MCP, using the same
target-scoped canonical path and lock ownership as a hook directory. Its name must then
be one path component; a token-free server keeps whatever name it has, because nothing
installs it as a directory.

## Hooks Compilation (`hooks/`)

Hook discovery and output loading. The `hook.toml` model, its reader, each target root's
placement and event table, the `[hooks]` renderers, fragment validation and token substitution
(`Tokens`) are `mars-schema::hooks`; the binary holds no copy of those rules. `load_output`
reads the fragment file when the hook declares one and asks the schema what the hook installs on
the target (`HookOutput`: merge events, or a list of files);
`check_hooks` is `mars check`'s hook pass (the `requires-mars >= 0.25.0` floor, `if` on a
non-tool event, what Claude and Codex leave out). This module reads
the bytes, calls the schema, and adds the location to its path-free diagnostics exactly once:
the real path for the project's own hook, the package and hook (with the version when reading
`hook.toml`) for a dependency's (whose path is a staging directory that must never reach an error).
Placement and lock recording are handled by `config_entries/`. Target construction derives the
filter-facing `DiscoveredItem` from that strict parse instead of parsing `hook.toml` again.

## Variants (`variants/`)

Skill variant layout validation, indexing, and projection per harness target.

## Visibility

`mars_schema::visibility::Visibility` is the one type. A hook's or MCP server's authored value is parsed once; an
unknown value warns (`warn_unknown_visibility`) and becomes `Local`; downstream selection only
compares the enum with `Exported`.

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
