# src/build/

Generates launch bundles consumed by harness runtimes and committed plugin trees consumed by
harness plugin installers.

## Contracts

### Plugin builds and publication

`build plugin` is a publish operation, not a project compilation. It constructs an implicit
consumer over `PublishLens`, resolves before staging (so relative package dependencies retain
their declared base), stages through resolve's shared rooted-package operation, and reuses sync's
`TargetState`/`RenameIndex` to flatten the graph. `PreparedTarget` carries the dependency
precedence used during rename application so tree writing cannot reconstruct a different order. It
must not read `mars.local.toml`, `mars.lock`, `.mars/`, project target configuration, model
caches, or installed harnesses. `plugin::output::replace_owned_tree` alone owns destination
replacement and sentinel publication.

The flatten step uses the sync roots of the requested trees. Five plugin harnesses map to three
tree identities: Claude/Codex/Grok → Claude, Cursor → Cursor, OpenCode → OpenCode. Tree profiles
derive their harness kind, canonical id, variant key and target root from the compiler descriptor;
they add only plugin-specific agent policy, native model aliases, hook-root token, config writer
and finish writer. Profiles also carry their typed `PluginTree` identity and apply a hook filter
before copying, so files, counts and finish-writer inputs agree. `tree::write_tree` is the sole item
loop. `tree::check_sources` scans authored agents, skills, hooks and the selected MCP union once;
per-tree writers report only lowering behavior. `TargetState::exported_hooks` retains filtered
exported graph hooks even when no requested tree selected a fragment, which is the input for plugin
omission diagnostics. `support::emit` receives the full target so graph-wide agent and bootstrap
counts never depend on one format's emitted components.

The build request carries the semantic destination instead of only a path. `Configured` resolves
`[plugin].dir` package-relative and includes the root-file plan; explicit `Out(path)` keeps today's
cwd-relative CLI path semantics and excludes repo-root files. Until the config schema owns the
`[plugin]` table, the configured form fails with a targeted error. `version: None` takes
`[package].version`; an override changes generated metadata and the sentinel without changing the
manifest used for resolution. Plugin-facing identity comes from `PackageMetadata`, while
`SourceName` must continue to come from `[package].name` because the plugin name may differ. The
report's `trees` array carries each typed tree's final output path and component counts rather than
flattening one preferred tree into the top level.

The output writer builds `<tree>/` directories directly into an empty sibling staging directory.
After it succeeds, the output layer hashes every tree, writes one root `.mars-plugin.json`, and
publishes the staged output directory with a two-rename swap. Publication and startup recovery share a cross-process lock keyed by the
absolute output path; non-empty trees without a valid sentinel are never replaced.
The root-package input fingerprint is computed from discovered authored item paths after recovery
and under that same lock, before the staging tree is populated, so crash leftovers and generated
catalog/output trees cannot make an otherwise clean checkout stale.

Agent `skills` references receive the plugin namespace before native lowering. Generated body
guidance therefore does not depend on the later fence-aware authored-prose rewrite to become
qualified.

The output API deliberately has no package-root parameter because explicit `--out` paths may
live elsewhere. It independently rejects filesystem roots, the current directory and its
ancestors, package roots, symlinks, and non-empty unowned trees.

### Two launch-bundle modes

1. **Ad-hoc mode** (no `--agent`):
   - `--model` is optional; when omitted, routing selects an installed/default
     harness and leaves `routing.model`, `routing.model_token`, and
     `routing.harness_model` empty so the harness can use its own default model
   - Works from a plain directory with **no `mars.toml`** — `can_run_without_project()`
     in `src/cli/mod.rs:280` supplies a synthetic `MarsContext` when project
     discovery fails and the exact ad-hoc condition is met
   - `empty_agent_profile()` is used (no agent file read, no skills, no profile tool rules)
   - Skills loading skipped (no `.mars/` store needed); profile tool resolution yields empty
     rules and flat lists, while `tools.capability_natives` still names the target harness's
     `agent` and `task` tools for platform rules

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

Route facts are recorded in `routing.harness_model_source` and
`routing.harness_model_confidence`. Build policy owns warnings for selected model
fallbacks and invalid resolved policy states it normalizes. Harness projection owns
reporting about what the selected harness can enforce. Tool permission gaps are never
warnings: they ride `tools.rules[].native_status` and `note`. Exhausted routing is an
error, never a warning followed by an unassessed replacement route.

Examples that are NOT warnings (they go to routing/provenance fields):
- `harness_model_source: "passthrough"` — Pi or explicit harness receives the model token as-is; this is expected behavior
- `harness_model_confidence: "unknown"` — Pi/passthrough harnesses are provider-routers at runtime; unknown confidence is the correct answer

### `can_run_without_project` guard

The guard applies only after project-root discovery returns `ProjectRootNotFound`. It permits
three explicit routes: ad-hoc launch-bundle (`--agent` absent), `build plugin` with an explicit
package path, and `models refresh-catalog`. Agent-mode launch-bundle and pathless plugin builds
still require `mars.toml` in an ancestor directory.

### Warning accumulation pipeline

```
parse_diags           ← frontmatter parse warnings (agent mode only)
policy.warnings       ← catalog refresh issues, model fallback, unenforceable execution policy
prompt.warnings       ← missing skills
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

### Literal model pins

`--model <id> --literal-model` bypasses alias lookup and model backups while
retaining normal harness permission, support and auth assessment. A provider-qualified
literal retains its constraint in `routing.provider_constraint`. An explicitly empty
literal pins harness-default mode instead of falling through to project/profile defaults.
Continuation consumers use this to revalidate a recorded canonical selection without
allowing a renamed alias to change the model.

### Selection report and errors

Bundle version 4 carries report version 3 at `routing.route_trace`. Policy resolution
accumulates every attempted model and its harness assessments, preserving scope and
winning target-file provenance. `selected` indexes the accepted assessment, including
an earlier deferred attempt. The report is diagnostic; execution uses routing fields.
An authored supported preference can stop selection even when its auth is
`auth_unchecked` or native `auth_unknown`; the model-attempt loop does not
defer that route to an eligible backup. A failed gated listing is never promoted.

JSON failures expose `error: {code, message}` and a top-level `route_trace` once
selection began. Configuration errors before selection have no report. Configuration
errors during prompt/skill/tool construction retain the selected report: `selected`
means route selected, not bundle constructed or process started. Other error handling
and exit codes remain unchanged.

### `harness_model` vs `candidate_slugs`

`LaunchBundle.routing.harness_model` is the **runtime model id** for the selected harness
(including Cursor effort baking in `runnable.rs`). `routing.candidate_slugs` copies assessment
probe/catalog candidates for debugging only — bundle doc comment: consumers run `harness_model`
verbatim. `routing.candidate_slugs` on the trace/report is diagnostic; do not use it for launch.

### Alias `provider` → `harness_model`

Resolved from the selected assessment by `models::resolve_harness_model`
([`src/models/.context/CONTEXT.md`](../../models/.context/CONTEXT.md)):

- **Codex / Claude:** emit the **requested spelling**, never a normalized catalog
  `chosen_model` (`claude-opus-4.6` remains `claude-opus-4.6`).
- **Pi / OpenCode:** use the slug already selected by routing
  (`openai-codex/gpt-5.4-mini`, etc.); `provider_constraint` filters the
  assessment and only qualifies a fallback when no selected slug exists.

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
LaunchBundleRequest {agent, model, harness, effort, approval, sandbox, network, network_allow, network_deny, extra_skills}
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
           ├─ execution::resolve_execution_policy()    (effort, approval, sandbox, network, autocompact)
           └─ runnable::resolve_routing()              (populate Routing with aggregate report)
       │
       ├─ resolve_effective_skills()  (profile skills filtered by harness kind)
       ├─ compile_prompt_surface()    (system instruction + supplemental docs + inventory)
       └─ resolve_bundle_tools()      (profile rules + platform capability natives per harness)
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

- [policy/AGENTS.md](../policy/AGENTS.md) — policy resolution pipeline, field independence,
  cross-field precedence conflict handling
- [policy/.context/CONTEXT.md](../policy/.context/CONTEXT.md) — preference ordering, independent
  pins, and model-attempt exhaustion
- [src/models/AGENTS.md](../../models/AGENTS.md) — catalog `ensure_fresh`, `ModelsRefreshControl`
- [src/routing/.context/CONTEXT.md](../../routing/.context/CONTEXT.md) — harness candidate
  evaluation, selection-kind vs match-evidence semantics, and `RouteDecisionReport`
  serialization surface
- [src/harness/.context/CONTEXT.md](../../harness/.context/CONTEXT.md) — harness registry,
  capability snapshot, probe integration
- [src/cli/build.rs](../../cli/build.rs) — CLI arg definitions and entry points
- [tests/smoke/manual/results-launch-bundle-resolver.md](../../../tests/smoke/manual/results-launch-bundle-resolver.md) —
  smoke evidence for routing and warning behavior
