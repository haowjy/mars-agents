# src/build/ — Runtime and Distribution Artifacts

Generates serializable launch bundles for harness runtimes and committed plugin trees for
package distribution.

## Plugin Builds

`plugin/` builds one Claude-format tree from the consumer view of a package's complete
publish graph. It resolves through `PublishLens`, stages every package, collects the flattened
target with sync's shared collect/rename/validate pipeline, then lowers agents, skills, exported
hooks, and exported MCP servers without reading project config, a lockfile, `.mars/`, or host
model state. The implicit target selection includes Claude and Codex so warnings about hooks that
the shared tree cannot carry come from the parsed `TargetState`, not a second manifest scan.

The generated tree is handed to `plugin::output::replace_owned_tree`; the build core never
mutates the destination itself. The output layer stages in a sibling directory, records a
`.mars-plugin.json` sentinel, and publishes with a two-rename swap under a cross-process lock.
It recovers interrupted swaps before a new build and never replaces a non-empty destination
without a valid sentinel. Reference qualification uses each source item's `RenameIndex` view
before applying the building package's single plugin namespace.

`plugin_inputs_hash` is the offline F3 seam: package-owned source bytes plus `[package]` without
`version` and the `[plugin]`, `[dependencies]`, `[models]`, `[skills]`, and `[agents]` tables.
It never resolves. The sentinel separately records resolved dependency version/commit identities
and one SHA-256 per generated file; those file hashes are the only output-integrity source.

## Mental Model

```
LaunchBundleRequest {agent, fallback_root, model, harness, effort, approval, sandbox, network, network_allow, network_deny, extra_skills}
    ↓
[agent mode] read root then fallback .mars/agents/<name>.md → AgentProfile + body
[ad-hoc mode] empty_agent_profile() + "" body
    ↓
resolve_policy() → Routing + ExecutionPolicy + Provenance
    ↓
resolve_effective_skills() + compile_prompt_surface() + resolve_bundle_tools()
    ↓
LaunchBundle {routing, execution_policy, prompt_surface, tools, skills, provenance, warnings}
```

## Two Modes

### Ad-hoc Mode (no `--agent`)
- Works from a plain directory with **no `mars.toml`**
- `--model` is optional; when omitted, the selected harness receives an empty
  model field and uses its own default model
- `can_run_without_project()` in `src/cli/mod.rs` supplies synthetic `MarsContext`
- No skills loaded and no profile tools resolved (empty rules and flat lists). The target's
  `agent` / `task` entries in `tools.capability_natives` are still present for platform rules.

### Agent/Profile Mode (`--agent <name>`)
- Reads `.mars/agents/<name>.md` from `ctx.project_root` first
- With `fallback_root`, missing agents and skills come from that root's `.mars/`
  store; project config, aliases, caches, targets, and native manifests remain
  primary-root-only
- Requires synced project (`mars.toml` present, `mars sync` already run)
- Full profile parsing + skill loading + tool resolution

## Warning Semantics

`LaunchBundle.warnings[]` contains **user-actionable degraded states only**.

**REAL warnings** (user can act on them):
- Linked harness constraints blocked auto-routing
- An `allowlist` mode with no allowed hosts failed closed to `deny`

**NOT warnings** (go to routing/provenance fields, or to a `tools.rules[].note`):
- Permission gaps: a tool name Mars does not recognize, a harness with no switch for a tool,
  an MCP ref a harness cannot represent, an allow/deny overlap. Meridian prints `warnings[]`
  at every launch; `native_status: unknown` plus `note` is the only channel, and `mars sync`
  tells the author about a typo.
- `harness_model_source: "passthrough"` — expected behavior
- `harness_model_confidence: "unknown"` — correct answer for passthrough harnesses

Each profile `tools.rules[]` entry keeps permission emission (`native`) separate from callable
tool availability (`native_tools`). Both come from `EffectiveToolPolicy::project`; build code
must not strip patterns or otherwise re-derive one from the other. Under a `*: deny` allowlist,
a launcher derives availability from known `allow` and `ask` rules, never `deny` rules.

Policy resolution emits warnings for model fallback. Model/harness conflicts never
clear the requested model. Route facts go to `routing.harness_model_source` and
`routing.harness_model_confidence`.

## Policy Resolution Pipeline

`resolve_policy()` resolves routing, execution, and provenance. Each field resolves
independently — see [`policy/AGENTS.md`](policy/AGENTS.md) for the full pipeline,
field independence, preference ordering and pin handling.

`resolve_policy` always uses the catalog refresh policy on `.mars/`: fresh and stale
usable caches return immediately (stale schedules a detached refresh), while cold
cache and explicit `--refresh-models` fetch synchronously. Last-good failures warn.
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
- `policy/AGENTS.md` — policy resolution pipeline, field independence, cross-field precedence
- `src/routing/AGENTS.md` — harness candidate evaluation
- `src/cli/build.rs` — CLI arg definitions
