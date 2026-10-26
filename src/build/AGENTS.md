# src/build/ — Runtime and Distribution Artifacts

Generates serializable launch bundles for harness runtimes and committed plugin trees for
package distribution.

## Plugin Builds

`plugin/` builds selected harness trees matching what `mars sync` installs in the package's own
checkout: its items plus dependencies under its own filters, renames, and visibility rules. It
resolves through `PublishLens`, stages every package, collects the flattened target once with sync's
shared collect/rename/validate pipeline, then runs the one `tree::write_tree` loop for each
`TreeProfile`. `spec.rs` owns both declarative registries: `PLUGIN_HARNESS_SPECS`, one row per
consumer harness (tree, naming rule, display, README note, verification evidence), and
`TREE_PROFILES`, one row per published tree defined beside its writer in `tree/`. A harness row
points straight at its `TreeProfile`, which carries the directory key, root file, dropped agent
fields, lossiness adjustment, the plugin-root mapping, and the agent/config/finish writers, so a
harness and its tree cannot disagree and adding a harness is one row. `TreeProfile.key` is the
single key for both staged directory names and reported output paths. `RootFile` and `Finish` are
enums: a marketplace cannot be declared without its catalog directory and name rule, and the
standard finish owns its manifest path. `GENERATED_ROOT_FILES` in `plugin_metadata.rs` is the one
declared list of generated root files; staging and sentinel validation read it, and a tree row names
its constant directly, with `spec`'s test failing if a tree's root file or catalog directory is not
in that list. `PluginRoot` is an enum: a tree either writes a harness token literally or keeps the
Mars token for its own runtime to substitute, so the merged-hook path mapping matches on it instead
of unwrapping an optional token. Every selected hook and MCP server directory is copied whole into
the tree, and `${MARS_HOOK_DIR}` / `${MARS_MCP_DIR}` map through that same plugin root.
Harness identity, variant key, and sync root come from `compiler::harness_descriptor`; authored-hook
checks run once before copying. Claude, Codex, and Grok share the Claude tree; Cursor and OpenCode
have their own tree identities. The OpenCode tree is an npm-shaped package whose fixed `index.mjs`
registers skills and MCP servers and imports every targeted hook fragment as an in-memory TypeScript
data URL. A build warning reports fragments with no recognizable default export, but runtime
isolation decides whether each hook loads. Hook-directory substitution is JavaScript-string encoded,
and one fragment's load or setup failure does not block later hooks.

Publication staging materializes each graph node's prepared package — manifest, definitions and
listed payload — then copies the binary-owned hook, MCP, bootstrap and source-manifest surfaces. A
manifestless foreign source materializes in the schema v1 layout `normalize` gave it, so rediscovery
does not require excluded paths. Target collection applies the graph filters, except that every hook
root remains available for the strict pre-filter compatibility check. The input fingerprint walks
the root package's discovered item and MCP server-directory inventory separately; the single
authored walker confines links to the package and rejects special files there, while sync and
staging's binary-owned subtree copies retain sync's historical follow behavior. The fingerprint
includes inventory kinds and authored bytes, but never Mars-generated catalog metadata. A flat-root
skill retains authored root resources but its publication inventory excludes standard tooling
directories and top-level special filesystem nodes. Installed skill frontmatter is written once into
graph staging before every tree projects it; derived skill and hook trees then use the plain copier,
with other-target hook fragments excluded.

Plugin builds lower agents, skills, exported hooks, and exported MCP servers without reading project
config, a lockfile, `.mars/`, or host model state. The implicit target selection is the set of built
trees' sync roots. `TargetState` also retains every filtered exported graph hook, including hooks no
built root selected. A source check before the tree loop emits authored-content diagnostics,
including skill-schema findings, once; tree writing emits only lowering diagnostics. Support
diagnostics consume the full `TargetState` for graph-wide counts rather than inferring them from one
tree. `support.rs` derives component support from each listed harness's tree profile and the harness
row's agent evidence, so a row without evidence has no agent surface; verification evidence and an
optional prose note live on the harness spec rows. Each README support cell is that computed state
plus the note, so the prose can qualify a finding but never contradict it, and the README freshness
test renders the same tables. The Claude plugin MCP writer emits same-name environment pass-throughs
as `env_vars` for Claude/Codex/Grok compatibility; renamed references retain Claude's `${VAR}` form
and warn that Codex passes it literally. Sync continues to use the native Claude writer.

The Cursor profile lowers native Cursor agents without an additional plugin field filter, uses no
native model aliases, and writes `.cursor-plugin/plugin.json`, `mcp.json`, and versioned
`hooks/hooks.json` with `${CURSOR_PLUGIN_ROOT}` hook paths. Its MCP and hook serializers are the
same path-parameterized writers used by `.cursor` sync.

`PluginBuildRequest` carries validated `PackageMetadata` plus destination intent. `Configured`
resolves its validated `[plugin].dir` and enables the root-file plan, while `Out(path)` is the
explicit CLI output and never writes repo-root files. Validation applies `[plugin]` defaults only at
this boundary: directory `plugin`, identity and description from `[package]`, and all five harness
specs. `Configured` still requires the table, and explicit output cannot resolve to the configured
directory. The request's optional version overrides only generated metadata; `None` uses
`[package].version`. The publish graph's root source continues to use `[package].name` because the
plugin name may differ. Build reports contain one typed tree record and output path per generated
tree; human output prints one success line per record.

Config loading keeps the `plugin` value and package plugin metadata raw. `validate_plugin_build` is
the one semantic conversion to typed authors, canonical configured directory, requested harness
specs, plugin name, and catalog marketplace. The same rules therefore apply to configured and
explicit `--out` builds without blocking sync or list; `mars version` invokes that validation for
packages declaring `plugin` even when output is skipped, and `mars check` reports its error as a
warning.

The generated trees are nested at `<out>/<tree>/` and handed to `plugin::output::stage`; the build
core never mutates the destination itself. The staged build holds the output lock and root-file
plan until publication applies the plan and calls `StagedPlugin::publish`. Configured builds plan
the Claude/Cursor catalogs and OpenCode root package; explicit `--out` builds plan none. The shared
offline `inspect` classifies the owned directory, its fingerprint/version, and every recorded or
expected root file. Sentinel ownership is limited to the three generated root paths and records a
catalog directory only when Mars created its previously missing parent. Publication swaps the tree
and sentinel first — a rename swap that either completes or restores the previous tree, so a
failure there leaves package root files untouched — then rewrites the root files. The sentinel
records the root hashes that step replaces (`root_files_previous`), and only for paths whose bytes
it actually replaces or deletes, so a build that dies between the two steps is resumed by the next
build instead of refused. Root publication accepts an absent path, a file matching the sentinel's
current hash, a file still matching the previous build's output (classified `PreviousBuild`, which
the build may replace or delete and `mars check` reports as an unfinished publication), or bytes
identical to the new build; it removes recorded files plus owned, now-empty catalog directories
retired by a narrowed harness set. A path only the previous build recorded counts only while its
bytes still match, so a user's own file at a retired path is neither refused nor deleted.
Configured-build planning also rejects final staged or root-file paths that remain ignored after
gitignore negation.
The output
layer stages the whole output in a sibling directory, records one root
`<out>/.mars-plugin.json` sentinel, and publishes with a two-rename swap under the lock.
It recovers interrupted swaps before a new build, always removes the reserved real-directory
staging sibling under the output lock, refuses a symlink or non-directory at that path, and never
replaces a non-empty destination without a valid sentinel.
Reference qualification uses each source item's `RenameIndex` view
before applying the building package's single plugin namespace. Agent skill names are qualified
before native lowering so generated guidance is born qualified; the fence-aware prose pass is
only responsible for authored body references.

`plugin_inputs_hash` is the offline F3 seam: bytes under the root package's discovered own-item
paths (including hook fragments and MCP server directories), with generated trees skipped and text
line endings normalized, plus `[package]` without `version` and the `[plugin]`, `[dependencies]`,
`[models]`, `[skills]`, and `[agents]` tables. It never resolves. Builds compute it after output
recovery while holding the output lock. The sentinel separately records resolved dependency
version/commit identities, one SHA-256 per generated file, and the root-file ownership map; the
generated-file hashes are the only output-integrity source.

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
